//! Type, effect-row, and signature resolution workflows.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID, symbol_kind::SymbolKind, syntax::get_type_parameter_list_syntax,
};
use rayc_syntax::{
    effect_row::EffectRow as EffectRowSyntax,
    path::{Path, PathSegment},
    r#type::{Primitive as PrimitiveSyntax, Type as TypeSyntax},
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarMap},
    subst::Subst,
    ty::{Integer, Mutability, Primitive, Ty, TyKind, args::Args, lifetime::Lifetime},
};

use crate::{
    lifetime::elided_reference_lifetime_span,
    path::{PathResolution, TraitMemberParent},
    resolver::Resolver,
};

#[derive(Debug, Clone)]
pub struct ResolvedParameter {
    pub span: RelativeSpan,
    pub ty: Interned<Ty>,
}

/// The resolved semantic types and polymorphic environment of a function
/// signature.
#[derive(Debug, Clone)]
pub struct SignatureResolution {
    pub parameters: Vec<ResolvedParameter>,
    pub return_type: Interned<Ty>,
}

impl Resolver<'_> {
    fn infer_type_arguments(
        &mut self,
        identifier: &rayc_syntax::Identifier,
        expected: &[TyKind],
    ) -> Vec<Interned<Ty>> {
        if expected.is_empty() {
            return Vec::new();
        }

        let mut inferred = Vec::with_capacity(expected.len());
        for kind in expected {
            // Type inference ignores lifetimes, so an omitted lifetime in a
            // body is erased instead of inferred.
            if *kind == TyKind::Lifetime && self.infers() {
                inferred.push(Ty::new_lifetime(Lifetime::Erased, self.engine()));
                continue;
            }

            let Some(ty) = self.new_inference_type(*kind, identifier.span()) else {
                self.report_type_inference_not_allowed(identifier, expected.len());
                return expected.iter().map(|kind| self.new_error_type(*kind)).collect();
            };
            inferred.push(ty);
        }

        inferred
    }

    async fn resolve_explicit_type_arguments(
        &mut self,
        path: &PathSegment,
        expected: &[TyKind],
    ) -> Vec<Interned<Ty>> {
        let span = path
            .arguments()
            .map(|arguments| arguments.span())
            .expect("type arguments should be present");

        let mut resolved = Vec::new();

        if let Some(arguments) = path.arguments() {
            for (index, argument) in arguments.type_arguments().enumerate() {
                let ty = if let Some(expected_kind) = expected.get(index) {
                    Box::pin(self.resolve_type_term(&argument, *expected_kind)).await
                } else {
                    Box::pin(self.infer_type_term(&argument)).await
                };

                resolved.push(ty);
            }
        }

        if resolved.len() != expected.len() {
            self.report_type_argument_arity_mismatch(span, expected.len(), resolved.len());
        }

        resolved.truncate(expected.len());
        resolved.extend(expected[resolved.len()..].iter().map(|kind| self.new_error_type(*kind)));
        resolved
    }

    async fn resolve_given_dictionary(&mut self, path: &Path) -> Interned<Ty> {
        let Ok(resolution) = self.resolve_path(path).await else {
            return self.new_error_type(TyKind::Instance);
        };

        match resolution {
            PathResolution::SelfInstance(instance) => {
                self.engine().intern(Ty::SelfInstance(instance))
            }
            PathResolution::Instance(instance) => {
                self.new_instance_type(instance.symbol_id(), instance.args().clone())
            }
            PathResolution::PolyVar(poly_var_id) => {
                let ty = self.new_poly_var_type_from_id(poly_var_id);
                let actual = self.type_kind(&ty).await;
                if actual == TyKind::Instance {
                    ty
                } else {
                    self.report_type_kind_mismatch(path.span(), TyKind::Instance, actual);
                    self.new_error_type(TyKind::Instance)
                }
            }
            resolution => {
                if let Some(actual) = resolution.symbol_kind() {
                    self.report_expected_instance(path.span(), actual);
                }
                self.new_error_type(TyKind::Instance)
            }
        }
    }

    async fn resolve_given_arguments(
        &mut self,
        path: &PathSegment,
        symbol_id: GlobalSymbolID,
        parameters: Option<&PolyVarMap>,
        type_parameter_count: usize,
        subst: &mut Subst,
    ) -> Vec<Interned<Ty>> {
        let given_parameter_count =
            parameters.map_or(0, |parameters| parameters.len() - type_parameter_count);
        let explicit_given_count = parameters
            .into_iter()
            .flat_map(PolyVarMap::iter)
            .skip(type_parameter_count)
            .filter(|(_, parameter)| parameter.is_source())
            .count();
        let mut supplied = vec![None; given_parameter_count];
        let mut positional_index = 0;
        let mut saw_named = false;

        if let Some(arguments) = path.supplied_given_arguments() {
            for argument in arguments.given_arguments() {
                let Some(dictionary) = argument.dictionary() else { continue };

                if let Some(argument_name) = argument.name() {
                    saw_named = true;
                    let Some(name) = argument_name.name() else { continue };
                    let Some(index) = parameters
                        .into_iter()
                        .flat_map(PolyVarMap::iter)
                        .skip(type_parameter_count)
                        .position(|(_, expected)| {
                            expected.is_source() && **expected.name() == *name.kind.0
                        })
                    else {
                        self.report_given_argument_not_found(name.kind.0.clone(), name.span());
                        continue;
                    };
                    if let Some((_, original_span)) = &supplied[index] {
                        self.report_duplicate_given_argument(
                            name.kind.0.clone(),
                            *original_span,
                            name.span(),
                        );
                        continue;
                    }
                    supplied[index] = Some((dictionary, name.span()));
                    continue;
                }

                if saw_named {
                    self.report_positional_given_argument_after_named(argument.span());
                }

                if positional_index < explicit_given_count {
                    supplied[positional_index] = Some((dictionary, argument.span()));
                } else {
                    self.report_too_many_given_arguments(argument.span(), explicit_given_count);
                }
                positional_index += 1;
            }
        }

        let mut arguments = Vec::with_capacity(given_parameter_count);
        // Resolve the supplied syntax and create missing inferences in parameter order.
        // Each binding may occur in the requirement of a later given parameter.
        for (supplied, (parameter_id, parameter)) in supplied
            .into_iter()
            .zip(parameters.into_iter().flat_map(PolyVarMap::iter).skip(type_parameter_count))
        {
            let expected_trait_ref = self.apply_subst_to_trait_ref(
                parameter.trait_ref().expect("a given parameter must have an instance requirement"),
                subst,
            );
            let value = if let Some((dictionary, _)) = supplied {
                let instance = Box::pin(self.resolve_given_dictionary(&dictionary)).await;
                self.require_instance_trait_ref(
                    instance.clone(),
                    expected_trait_ref,
                    dictionary.span(),
                );
                instance
            } else {
                self.new_instance_inference_type(&expected_trait_ref, path.span()).unwrap_or_else(
                    || {
                        self.report_missing_given_argument(parameter.name().clone(), path.span());
                        self.new_error_type(TyKind::Instance)
                    },
                )
            };

            self.compose_subst(
                subst,
                &Subst::new_singleton(GlobalPolyVarID::new(symbol_id, parameter_id), value.clone()),
            );
            arguments.push(value);
        }
        arguments
    }

    /// Returns how many of the leading type parameters of `symbol_id` explicit
    /// type arguments instantiate, or `None` when they are always inferred.
    ///
    /// A definition that declares a type-parameter list accepts arguments for
    /// exactly the parameters it declares. They precede its generated ones,
    /// such as fresh elided lifetimes and callable types, which are always
    /// inferred. A definition without one accepts none, as its parameter
    /// types introduce its type parameters. Any other symbol accepts arguments
    /// for all `type_parameter_count` type parameters.
    async fn explicit_type_parameter_count(
        &self,
        symbol_id: GlobalSymbolID,
        parameters: Option<&PolyVarMap>,
        type_parameter_count: usize,
    ) -> Option<usize> {
        if !self.symbol_kind(symbol_id).await.has_optional_type_parameter_list() {
            return Some(type_parameter_count);
        }
        self.engine().get_type_parameter_list_syntax(symbol_id).await?;

        Some(
            parameters
                .into_iter()
                .flat_map(PolyVarMap::iter)
                .take(type_parameter_count)
                .take_while(|(_, parameter)| parameter.is_source())
                .count(),
        )
    }

    pub(crate) async fn resolve_arguments(
        &mut self,
        symbol_id: GlobalSymbolID,
        path: &PathSegment,
        identifier: &rayc_syntax::Identifier,
        parameters: Option<&PolyVarMap>,
        mut subst: Subst,
    ) -> Args {
        // Type, effect and lifetime parameters precede dictionaries.
        let type_parameter_count = parameters
            .into_iter()
            .flat_map(PolyVarMap::iter)
            .take_while(|(_, parameter)| parameter.kind() != TyKind::Instance)
            .count();
        let type_kinds = parameters
            .into_iter()
            .flat_map(PolyVarMap::iter)
            .take(type_parameter_count)
            .map(|(_, parameter)| parameter.kind())
            .collect::<Vec<_>>();

        // Resolve the explicit type arguments and infer the rest.
        let explicit_count =
            self.explicit_type_parameter_count(symbol_id, parameters, type_parameter_count).await;
        let has_explicit_type_arguments = path.has_explicit_type_arguments();

        let mut resolved = match explicit_count {
            Some(explicit_count) if has_explicit_type_arguments => {
                let (explicit, generated) = type_kinds.split_at(explicit_count);
                let mut resolved = self.resolve_explicit_type_arguments(path, explicit).await;
                resolved.extend(self.infer_type_arguments(identifier, generated));
                resolved
            }
            Some(_) => self.infer_type_arguments(identifier, &type_kinds),
            None => {
                if has_explicit_type_arguments
                    && let Some(span) = path.arguments().map(|arguments| arguments.span())
                {
                    self.report_explicit_type_arguments_not_allowed(span);
                }
                self.infer_type_arguments(identifier, &type_kinds)
            }
        };

        // Given arguments are checked against requirements that mention the
        // type arguments just resolved.
        let own_subst = parameters
            .into_iter()
            .flat_map(PolyVarMap::iter)
            .take(type_parameter_count)
            .zip(&resolved)
            .map(|((parameter_id, _), argument)| {
                (GlobalPolyVarID::new(symbol_id, parameter_id), argument.clone())
            })
            .collect();
        self.compose_subst(&mut subst, &own_subst);
        resolved.extend(
            self.resolve_given_arguments(
                path,
                symbol_id,
                parameters,
                type_parameter_count,
                &mut subst,
            )
            .await,
        );
        self.new_args(resolved)
    }
}

impl Resolver<'_> {
    /// Resolves effect-row syntax relative to this resolver's site.
    #[must_use]
    pub async fn resolve_effect_row(&mut self, syntax: &EffectRowSyntax) -> Interned<Ty> {
        match syntax {
            EffectRowSyntax::Path(path) => {
                Box::pin(self.resolve_type_path(path, TyKind::EffectRow)).await
            }
            EffectRowSyntax::ConcreteEffectRow(effect_row) => {
                let mut labels = Vec::new();

                for path in effect_row.effects() {
                    let Ok(path_resolution) = self.resolve_effect_path(&path).await else {
                        continue;
                    };
                    labels.push(self.new_effect_label(
                        path_resolution.symbol_id(),
                        path_resolution.args().clone(),
                    ));
                }

                let tail =
                    if let Some(variable) = effect_row.tail().and_then(|tail| tail.variable()) {
                        Some(Box::pin(self.resolve_type_path(&variable, TyKind::EffectRow)).await)
                    } else {
                        None
                    };

                self.new_effect_row_type(labels, tail)
            }
        }
    }

    /// Resolves one type against this resolver's polymorphic environment.
    #[must_use]
    pub async fn resolve_type(&mut self, syntax: &TypeSyntax) -> Interned<Ty> {
        self.resolve_type_term(syntax, TyKind::Star).await
    }

    /// Resolves a type term and validates its kind, including polymorphic
    /// variables and associated projections. Recovery errors have
    /// `expected_kind` too.
    pub async fn resolve_type_term(
        &mut self,
        syntax: &TypeSyntax,
        expected_kind: TyKind,
    ) -> Interned<Ty> {
        let ty = Box::pin(self.synthesize_type_term(syntax, expected_kind)).await;
        self.check_type_kind(ty, syntax.span(), expected_kind).await
    }

    /// Resolves a term whose result kind is not yet known, as on the left of a
    /// where equality. Nested components still satisfy their required kinds.
    pub async fn infer_type_term(&mut self, syntax: &TypeSyntax) -> Interned<Ty> {
        Box::pin(self.synthesize_type_term(syntax, TyKind::Star)).await
    }

    async fn check_type_kind(
        &self,
        ty: Interned<Ty>,
        span: RelativeSpan,
        expected: TyKind,
    ) -> Interned<Ty> {
        // Preserve the required kind during recovery without duplicating a resolution
        // error.
        if matches!(&*ty, Ty::Application(application) if matches!(application.view(), rayc_type::ty::application::View::Error))
        {
            return self.new_error_type(expected);
        }
        let actual = self.type_kind(&ty).await;
        if actual == expected {
            ty
        } else {
            self.report_type_kind_mismatch(span, expected, actual);
            self.new_error_type(expected)
        }
    }

    // Shared synthesis preserves the actual result kind until the checking
    // boundary.
    async fn synthesize_type_term(
        &mut self,
        syntax: &TypeSyntax,
        recovery_kind: TyKind,
    ) -> Interned<Ty> {
        match syntax {
            TypeSyntax::EffectRow(row) => {
                Box::pin(self.resolve_effect_row(&EffectRowSyntax::ConcreteEffectRow(row.clone())))
                    .await
            }
            TypeSyntax::Primitive(primitive) => {
                let primitive = match primitive {
                    PrimitiveSyntax::Int8(_) => Primitive::Integer(Integer::Int8),
                    PrimitiveSyntax::Int16(_) => Primitive::Integer(Integer::Int16),
                    PrimitiveSyntax::Int32(_) => Primitive::Integer(Integer::Int32),
                    PrimitiveSyntax::Int64(_) => Primitive::Integer(Integer::Int64),
                    PrimitiveSyntax::Isize(_) => Primitive::Integer(Integer::Isize),
                    PrimitiveSyntax::Uint8(_) => Primitive::Integer(Integer::Uint8),
                    PrimitiveSyntax::Uint16(_) => Primitive::Integer(Integer::Uint16),
                    PrimitiveSyntax::Uint32(_) => Primitive::Integer(Integer::Uint32),
                    PrimitiveSyntax::Uint64(_) => Primitive::Integer(Integer::Uint64),
                    PrimitiveSyntax::Usize(_) => Primitive::Integer(Integer::Usize),
                    PrimitiveSyntax::Bool(_) => Primitive::Bool,
                    PrimitiveSyntax::Float32(_) => Primitive::Float32,
                    PrimitiveSyntax::CInt(_) => Primitive::Integer(Integer::CInt),
                    PrimitiveSyntax::CStr(_) => Primitive::CStr,
                };
                self.new_primitive_type(primitive)
            }
            TypeSyntax::Pointer(ptr) => {
                let pointee = if let Some(pointed_type) = ptr.pointed_type() {
                    Box::pin(self.resolve_type(&pointed_type)).await
                } else {
                    self.new_error_type(TyKind::Star)
                };
                let mutability = if ptr.mut_keyword().is_some() {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                };
                self.new_pointer_type(pointee, mutability)
            }
            TypeSyntax::Reference(reference) => {
                let lifetime = if let Some(lifetime) = reference.explicit_lifetime() {
                    self.resolve_lifetime(&lifetime).await
                } else {
                    self.elided_lifetime(elided_reference_lifetime_span(reference)).await
                };
                let pointee = if let Some(pointed_type) = reference.pointed_type() {
                    Box::pin(self.resolve_type(&pointed_type)).await
                } else {
                    self.new_error_type(TyKind::Star)
                };
                let mutability = if reference.mut_keyword().is_some() {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                };
                self.require_reference_wf(pointee.clone(), lifetime.clone(), reference.span());
                Ty::new_reference(lifetime, pointee, mutability, self.engine())
            }
            TypeSyntax::Lifetime(lifetime) => self.resolve_lifetime(lifetime).await,
            TypeSyntax::Tuple(tuple) => {
                let mut arguments = Vec::new();
                for element in tuple.elements() {
                    arguments.push(Box::pin(self.resolve_type(&element)).await);
                }
                self.new_tuple_type(arguments)
            }
            TypeSyntax::Path(path) => self.synthesize_type_path(path, recovery_kind).await,
        }
    }

    async fn resolve_type_path(&mut self, path: &Path, expected_kind: TyKind) -> Interned<Ty> {
        let ty = Box::pin(self.synthesize_type_path(path, expected_kind)).await;
        self.check_type_kind(ty, path.span(), expected_kind).await
    }

    async fn synthesize_type_path(&mut self, path: &Path, recovery_kind: TyKind) -> Interned<Ty> {
        if let Some(identifier) = path.bare_identifier()
            && let Some(id) = self.search_poly_var(&identifier.kind.0)
        {
            return self.new_poly_var_type_from_id(id);
        }

        let Ok(resolution) = Box::pin(self.resolve_path(path)).await else {
            return self.new_error_type(recovery_kind);
        };

        if let PathResolution::Struct(struct_) = &resolution {
            return self.new_struct_type(struct_.symbol_id(), struct_.args().clone());
        }

        let projection = match &resolution {
            PathResolution::TraitMember(member)
                if resolution.symbol_kind() == Some(SymbolKind::TraitType) =>
            {
                match member.parent() {
                    TraitMemberParent::This(instance) => Some((
                        member.symbol_id(),
                        self.engine().intern(Ty::SelfInstance(*instance)),
                        member.args().clone(),
                    )),
                    TraitMemberParent::Named(_) => {
                        self.report_named_trait_type_projection(path.span());
                        return self.new_error_type(recovery_kind);
                    }
                }
            }
            PathResolution::UnresolvedInstanceMember(member)
                if resolution.symbol_kind() == Some(SymbolKind::TraitType) =>
            {
                Some((
                    member.trait_member_id(),
                    Ty::new_poly_var(member.instance(), self.engine()),
                    member.args().clone(),
                ))
            }
            PathResolution::ResolvedInstanceMember(member)
                if resolution.symbol_kind() == Some(SymbolKind::InstanceType) =>
            {
                use rayc_type::instance_member::get_instance_member;
                let Some(correspondence) =
                    self.engine().get_instance_member(member.symbol_id()).await
                else {
                    self.report_missing_trait_type_declaration(path.span());
                    return self.new_error_type(recovery_kind);
                };
                if self.symbol_kind(correspondence.trait_member_id()).await != SymbolKind::TraitType
                {
                    self.report_missing_trait_type_declaration(path.span());
                    return self.new_error_type(recovery_kind);
                }
                Some((
                    correspondence.trait_member_id(),
                    member.instance().clone(),
                    member.args().clone(),
                ))
            }
            PathResolution::Def(_)
            | PathResolution::ExternDef(_)
            | PathResolution::Module(_)
            | PathResolution::Effect(_)
            | PathResolution::Trait(_)
            | PathResolution::Instance(_)
            | PathResolution::Marker(_)
            | PathResolution::Struct(_)
            | PathResolution::PolyVar(_)
            | PathResolution::SelfInstance(_)
            | PathResolution::TraitMember(_)
            | PathResolution::ResolvedInstanceMember(_)
            | PathResolution::UnresolvedInstanceMember(_)
            | PathResolution::EffectOperation(_) => None,
        };
        if let Some((id, dictionary, args)) = projection {
            Ty::new_instance_associated(
                id,
                dictionary,
                args.interned_iter().cloned(),
                self.engine(),
            )
        } else {
            self.report_expected_value_type(path.span());
            self.new_error_type(recovery_kind)
        }
    }
}
