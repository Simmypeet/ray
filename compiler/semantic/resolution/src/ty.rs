//! Type, effect-row, and signature resolution workflows.

use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, symbol_kind::SymbolKind};
use rayc_syntax::{
    effect_row::{EffectRow as EffectRowSyntax, EffectRowAnnotation},
    path::{Path, PathSegment},
    r#type::{Primitive as PrimitiveSyntax, Type as TypeSyntax},
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVarMap},
    subst::Subst,
    ty::{Mutability, Primitive, Ty, TyKind, args::Args},
};

use crate::{
    is_poly_var_name,
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
                let mut ty = Box::pin(self.resolve_type(&argument)).await;

                if let Some(expected) = expected.get(index) {
                    let actual = self.type_kind(&ty).await;
                    if actual != *expected {
                        self.report_type_kind_mismatch(argument.span(), *expected, actual);
                        ty = self.new_error_type(*expected);
                    }
                }

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
                        .position(|(_, expected)| **expected.name() == *name.kind.0)
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

                if positional_index < supplied.len() {
                    supplied[positional_index] = Some((dictionary, argument.span()));
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

    pub(crate) async fn resolve_arguments(
        &mut self,
        symbol_id: GlobalSymbolID,
        path: &PathSegment,
        identifier: &rayc_syntax::Identifier,
        parameters: Option<&PolyVarMap>,
        mut subst: Subst,
    ) -> Args {
        let symbol_kind = self.symbol_kind(symbol_id).await;
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

        let type_arguments_are_implicit =
            matches!(symbol_kind, SymbolKind::Def | SymbolKind::TraitDef | SymbolKind::InstanceDef);
        let has_explicit_type_arguments = path.has_explicit_type_arguments();

        if type_arguments_are_implicit
            && has_explicit_type_arguments
            && let Some(span) = path.arguments().map(|arguments| arguments.span())
        {
            self.report_explicit_type_arguments_not_allowed(span);
        }

        let mut resolved = if type_arguments_are_implicit || !has_explicit_type_arguments {
            self.infer_type_arguments(identifier, &type_kinds)
        } else {
            self.resolve_explicit_type_arguments(path, &type_kinds).await
        };

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
            EffectRowSyntax::PolyVar(identifier) => {
                self.new_checked_poly_var_type(identifier, TyKind::EffectRow).await
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
                        Some(self.new_checked_poly_var_type(&variable, TyKind::EffectRow).await)
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
        match syntax {
            TypeSyntax::Primitive(primitive) => {
                let primitive = match primitive {
                    PrimitiveSyntax::Int32(_) => Primitive::Int32,
                    PrimitiveSyntax::Bool(_) => Primitive::Bool,
                    PrimitiveSyntax::Float32(_) => Primitive::Float32,
                    PrimitiveSyntax::CInt(_) => Primitive::CInt,
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
            TypeSyntax::Tuple(tuple) => {
                let mut arguments = Vec::new();
                for element in tuple.elements() {
                    arguments.push(Box::pin(self.resolve_type(&element)).await);
                }
                self.new_tuple_type(arguments)
            }
            TypeSyntax::Lambda(lambda) => {
                let mut parameters = Vec::new();
                if let Some(parameter_list) = lambda.parameters() {
                    for parameter in parameter_list.parameters() {
                        parameters.push(Box::pin(self.resolve_type(&parameter)).await);
                    }
                }
                let return_type = if let Some(return_type) = lambda.return_type() {
                    if let Some(return_type) = return_type.r#type() {
                        Box::pin(self.resolve_type(&return_type)).await
                    } else {
                        self.new_error_type(TyKind::Star)
                    }
                } else {
                    self.new_unit_type()
                };
                let effect_row_syntax = lambda.effect_row();
                let effect_row = match effect_row_syntax
                    .as_ref()
                    .and_then(EffectRowAnnotation::effect_row)
                {
                    Some(effect_row) => self.resolve_effect_row(&effect_row).await,
                    None if effect_row_syntax.is_some() => self.new_error_type(TyKind::EffectRow),
                    None => self.new_effect_row_type(Vec::new(), None),
                };
                self.new_lambda_type(parameters, return_type, effect_row)
            }
            TypeSyntax::Path(path) => self.resolve_type_path(path).await,
        }
    }

    async fn resolve_type_path(&mut self, path: &Path) -> Interned<Ty> {
        if let Some(identifier) = path.bare_identifier()
            && (is_poly_var_name(&identifier.kind.0)
                || self.search_poly_var(&identifier.kind.0).is_some())
        {
            return self.new_checked_poly_var_type(&identifier, TyKind::Star).await;
        }
        let Ok(resolution) = Box::pin(self.resolve_path(path)).await else {
            return self.new_error_type(TyKind::Star);
        };
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
                        return self.new_error_type(TyKind::Star);
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
                    return self.new_error_type(TyKind::Star);
                };
                if self.symbol_kind(correspondence.trait_member_id()).await != SymbolKind::TraitType
                {
                    self.report_missing_trait_type_declaration(path.span());
                    return self.new_error_type(TyKind::Star);
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
            self.new_error_type(TyKind::Star)
        }
    }
}
