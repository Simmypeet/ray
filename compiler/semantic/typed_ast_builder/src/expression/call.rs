use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_resolution::path::{Effect, PathResolution, TraitMemberParent};
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    symbol_kind::SymbolKind,
    syntax::is_variadic_def,
};
use rayc_syntax::expression::{Call as CallSyn, DirectCall as DirectCallSyn};
use rayc_type::{
    poly_var::{build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args, effect_row::EffectLabel, self_instance::SelfInstance},
};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, call::Call, tuple::Tuple};

use crate::{
    bind::Bind,
    diagnostic::{Diagnostic, MismatchedArgumentCount, MismatchedIndirectArgumentCount},
    tast_builder::TAstBuilder,
};

enum ResolvedCallTarget<'a> {
    Direct {
        function_id: GlobalSymbolID,
        symbol_kind: SymbolKind,
    },
    UnresolvedInstanceAssociated {
        /// The dictionary the call dispatches through.
        instance: Interned<Ty>,

        /// The trait reference `instance` implements: the trait enclosing the
        /// trait def, with the arguments it is called through.
        trait_ref: TraitRef,
        trait_def_id: GlobalSymbolID,
    },
    EffectOperation {
        effect: &'a Effect,
        operation_id: GlobalSymbolID,
    },
}

impl ResolvedCallTarget<'_> {
    const fn function_id(&self) -> GlobalSymbolID {
        match self {
            Self::Direct { function_id, .. } => *function_id,
            Self::UnresolvedInstanceAssociated { trait_def_id, .. } => *trait_def_id,
            Self::EffectOperation { operation_id, .. } => *operation_id,
        }
    }

    const fn symbol_kind(&self) -> SymbolKind {
        match self {
            Self::Direct { symbol_kind, .. } => *symbol_kind,
            Self::UnresolvedInstanceAssociated { .. } => SymbolKind::TraitDef,
            Self::EffectOperation { .. } => SymbolKind::EffectOperation,
        }
    }

    /// Returns the substitution instantiating the signature of this target,
    /// where `call_subst` instantiates the type parameters of the called
    /// symbol.
    ///
    /// The signature of a trait def also mentions the parameters of its
    /// enclosing trait and the trait's self dictionary, which `call_subst`
    /// leaves out: they are instantiated with the arguments of the trait
    /// reference the call goes through, and with its dictionary.
    async fn signature_subst(&self, call_subst: &Subst, engine: &TrackedEngine) -> Subst {
        match self {
            Self::Direct { .. } | Self::EffectOperation { .. } => call_subst.clone(),
            Self::UnresolvedInstanceAssociated { instance, trait_ref, .. } => {
                let trait_id = trait_ref.trait_id();
                let mut subst =
                    engine.build_subst_from_args(trait_id, trait_ref.args().interned_iter()).await;

                // Associated types in the signature must use the selected
                // dictionary.
                subst.insert(SelfInstance::new(trait_id), instance.clone());
                subst.compose(call_subst, engine);
                subst
            }
        }
    }

    /// Builds the call of this target with `arguments`, where `call_subst`
    /// instantiates the type parameters of the called symbol.
    ///
    /// # Panics
    ///
    /// Panics if `call_subst` does not instantiate exactly the type
    /// parameters of the trait def of a call through an unresolved instance:
    /// the dictionary tells those of the enclosing trait.
    async fn into_call(
        self,
        arguments: Vec<TypedExprID>,
        call_subst: Subst,
        engine: &TrackedEngine,
    ) -> Call {
        match self {
            Self::Direct { function_id, .. } => {
                Call::new_direct(function_id, arguments, call_subst)
            }
            Self::UnresolvedInstanceAssociated { instance, trait_def_id, .. } => {
                // Every mapping belongs to the trait def, and there are as
                // many as it has parameters, so none is missing either.
                let own_count = call_subst
                    .poly_var_mappings()
                    .filter(|(poly_var_id, _)| poly_var_id.parent_id() == trait_def_id)
                    .count();
                assert!(
                    own_count == call_subst.len()
                        && own_count == engine.get_poly_var_map(trait_def_id).await.len(),
                    "a call should instantiate exactly the parameters of its trait def"
                );

                Call::new_unresolved_instance_associated(
                    instance,
                    trait_def_id,
                    call_subst,
                    arguments,
                )
            }
            Self::EffectOperation { effect, operation_id } => {
                Call::new_effect_operation(effect.symbol_id(), operation_id, arguments, call_subst)
            }
        }
    }
}

impl TAstBuilder {
    async fn bind_call_arguments(&mut self, syn: &CallSyn) -> Vec<TypedExprID> {
        let mut arguments = Vec::new();

        if let Some(argument_list) = syn.arguments() {
            for argument in argument_list.expressions() {
                arguments.push(self.bind(argument).await);
            }
        }

        arguments
    }

    async fn build_path_direct_call(&mut self, syn: &DirectCallSyn) -> TypedExprID {
        let Some(path) = syn.path() else {
            return self.push_error_expression(syn.span()).await;
        };
        let Some(call) = syn.call() else {
            return self.push_error_expression(syn.span()).await;
        };

        if let Some(identifier) = path.bare_identifier()
            && self.lookup_name_binding(&identifier.kind.0).is_some()
        {
            let callee = self.bind(identifier).await;
            return self.build_lambda_call(callee, &call).await;
        }

        let arguments = self.bind_call_arguments(&call).await;
        let Ok(resolution) = self.resolve_path(&path).await else {
            return self
                .push_error_expression_with_expression_children(syn.span(), arguments)
                .await;
        };
        let (target, call_subst) = match &resolution {
            PathResolution::Def(def) => (
                ResolvedCallTarget::Direct {
                    function_id: def.symbol_id(),
                    symbol_kind: SymbolKind::Def,
                },
                def.substitution(self.engine()).await,
            ),
            PathResolution::ExternDef(def) => (
                ResolvedCallTarget::Direct {
                    function_id: def.symbol_id(),
                    symbol_kind: SymbolKind::ExternDef,
                },
                Subst::new_empty(),
            ),
            PathResolution::EffectOperation(operation) => (
                ResolvedCallTarget::EffectOperation {
                    effect: operation.effect(),
                    operation_id: operation.symbol_id(),
                },
                operation.substitution(self.engine()).await,
            ),
            PathResolution::TraitMember(def)
                if resolution.symbol_kind() == Some(SymbolKind::TraitDef) =>
            {
                let (instance, trait_ref) = match def.parent() {
                    TraitMemberParent::Named(trait_ref) => {
                        (self.infer_trait_instance(trait_ref, path.span()).await, trait_ref.clone())
                    }
                    TraitMemberParent::This(instance) => (
                        self.engine().intern(Ty::SelfInstance(*instance)),
                        instance.trait_ref(self.engine()).await,
                    ),
                };
                (
                    ResolvedCallTarget::UnresolvedInstanceAssociated {
                        instance,
                        trait_ref,
                        trait_def_id: def.symbol_id(),
                    },
                    self.engine()
                        .build_subst_from_args(def.symbol_id(), def.args().interned_iter())
                        .await,
                )
            }
            PathResolution::ResolvedInstanceMember(def)
                if resolution.symbol_kind() == Some(SymbolKind::InstanceDef) =>
            {
                (
                    ResolvedCallTarget::Direct {
                        function_id: def.symbol_id(),
                        symbol_kind: SymbolKind::InstanceDef,
                    },
                    def.substitution(self.engine()).await,
                )
            }
            PathResolution::UnresolvedInstanceMember(def)
                if resolution.symbol_kind() == Some(SymbolKind::TraitDef) =>
            {
                (
                    ResolvedCallTarget::UnresolvedInstanceAssociated {
                        instance: Ty::new_poly_var(def.instance(), self.engine()),
                        trait_ref: def.trait_ref().clone(),
                        trait_def_id: def.trait_member_id(),
                    },
                    self.engine()
                        .build_subst_from_args(def.trait_member_id(), def.args().interned_iter())
                        .await,
                )
            }
            resolution => {
                if let Some(symbol_id) = resolution.global_id() {
                    self.push_symbol_not_callable(symbol_id, path.span());
                }
                return self
                    .push_error_expression_with_expression_children(syn.span(), arguments)
                    .await;
            }
        };

        self.build_resolved_direct_call(target, arguments, call_subst, syn.span(), None).await
    }

    /// Builds the call of `target` with `arguments`, where `call_subst`
    /// instantiates the type parameters of the called symbol alone.
    async fn build_resolved_direct_call(
        &mut self,
        target: ResolvedCallTarget<'_>,
        mut arguments: Vec<TypedExprID>,
        call_subst: Subst,
        span: RelativeSpan,
        // Is `Some` when the call is a lambda call
        value_arguments: Option<&[TypedExprID]>,
    ) -> TypedExprID {
        let signature_subst = target.signature_subst(&call_subst, self.engine()).await;

        // Check arguments before reading the resulting signature: constraints may
        // resolve inference variables and associated types used by the call.
        self.check_resolved_call_arguments(
            &target,
            &mut arguments,
            &signature_subst,
            span,
            value_arguments,
        )
        .await;
        let (return_type, effect_row) =
            self.resolve_call_signature(&target, &signature_subst).await;

        // Construct the call and introduce its effect exactly once.
        let call = target.into_call(arguments, call_subst, self.engine()).await;
        let expr_id = self.insert_expression(TypedExprKind::Call(call), span, return_type).await;
        self.push_effect_introduction(expr_id, &effect_row).await;
        expr_id
    }

    async fn check_resolved_call_arguments(
        &mut self,
        target: &ResolvedCallTarget<'_>,
        arguments: &mut [TypedExprID],
        signature_subst: &Subst,
        span: RelativeSpan,
        value_arguments: Option<&[TypedExprID]>,
    ) {
        let parameters = self.engine().get_parameter_map(target.function_id()).await;
        self.check_call_argument_count(target, parameters.len(), arguments.len(), span).await;

        // Def.call's second parameter packages the source value-call arguments.
        // Keep their individual locations available for argument diagnostics.
        for (index, ((_, parameter), argument)) in parameters.iter().zip(arguments).enumerate() {
            let parameter_ty = parameter.ty().apply_subst_or_clone(signature_subst, self.engine());
            if index == 1
                && let Some(value_arguments) = value_arguments
            {
                self.check_callable_arguments(&parameter_ty, *argument, value_arguments, span)
                    .await;
            } else {
                *argument = self.coerce(*argument, &parameter_ty).await;
                self.push_function_call_constraint(&parameter_ty, *argument).await;
            }
        }
    }

    async fn check_call_argument_count(
        &mut self,
        target: &ResolvedCallTarget<'_>,
        expected: usize,
        found: usize,
        span: RelativeSpan,
    ) {
        let is_variadic = if matches!(target.symbol_kind(), SymbolKind::Def | SymbolKind::ExternDef)
        {
            self.engine().is_variadic_def(target.function_id()).await
        } else {
            false
        };

        // Variadic calls must still provide all fixed parameters.
        if (!is_variadic && expected != found) || (is_variadic && found < expected) {
            self.push_diagnostic(Diagnostic::MismatchedArgumentCount(
                MismatchedArgumentCount::builder()
                    .calling_symbol(target.function_id())
                    .expected(expected)
                    .found(found)
                    .span(span)
                    .build(),
            ));
        }
    }

    async fn check_callable_arguments(
        &mut self,
        parameter_ty: &Interned<Ty>,
        argument_tuple: TypedExprID,
        arguments: &[TypedExprID],
        span: RelativeSpan,
    ) {
        // When Args normalizes to a tuple, check each source argument directly.
        let normalized = self.latest_type(parameter_ty).await;
        if let Ty::Application(application) = &*normalized
            && let rayc_type::ty::application::View::Tuple(tuple) = application.view()
        {
            if tuple.args().len() != arguments.len() {
                self.push_diagnostic(Diagnostic::MismatchedIndirectArgumentCount(
                    MismatchedIndirectArgumentCount::builder()
                        .expected(tuple.args().len())
                        .found(arguments.len())
                        .span(span)
                        .build(),
                ));
            }
            for (expected, actual) in tuple.args().iter().zip(arguments) {
                self.push_function_call_constraint(expected, *actual).await;
            }
        } else {
            // An unresolved Args projection retains the ordinary tuple constraint.
            // This generally shouldn't happen, but it's a good defensive fallback
            self.push_function_call_constraint(parameter_ty, argument_tuple).await;
        }
    }

    async fn resolve_call_signature(
        &self,
        target: &ResolvedCallTarget<'_>,
        signature_subst: &Subst,
    ) -> (Interned<Ty>, Interned<Ty>) {
        let return_type = self.engine().get_return_type(target.function_id()).await;
        let return_type = return_type.apply_subst_or_clone(signature_subst, self.engine());

        // Operations introduce their enclosing effect; other calls use the
        // declaration's effect row instantiated with the selected arguments.
        let effect_row = match target {
            ResolvedCallTarget::EffectOperation { effect, .. } => {
                let label = self
                    .engine()
                    .intern(EffectLabel::new(effect.symbol_id(), effect.args().clone()));
                Ty::new_effect_row([label], None, self.engine())
            }
            ResolvedCallTarget::Direct { .. }
            | ResolvedCallTarget::UnresolvedInstanceAssociated { .. } => self
                .engine()
                .get_effect_row(target.function_id())
                .await
                .apply_subst_or_clone(signature_subst, self.engine()),
        };
        (return_type, effect_row)
    }

    fn push_symbol_not_callable(&mut self, symbol_id: GlobalSymbolID, span: RelativeSpan) {
        self.push_diagnostic(Diagnostic::SymbolNotCallable(
            crate::diagnostic::SymbolNotCallable::builder().name(symbol_id).span(span).build(),
        ));
    }

    pub async fn build_lambda_call(&mut self, callee: TypedExprID, syn: &CallSyn) -> TypedExprID {
        // Bind once, preserving callee-before-arguments evaluation order.
        let arguments = self.bind_call_arguments(syn).await;
        let span = self.span_of_expression(callee).join(&syn.span());

        // Preserve the original inference terms in the signature substitution;
        // dictionary resolution applies the current inference substitution itself.
        let callee_ty = self.type_of_expression(callee);
        let trait_id = self.engine().get_core_item(CoreItem::DefTrait).await;
        let trait_ref = TraitRef::new(trait_id, Args::new([callee_ty], self.engine()));
        let instance = self.infer_trait_instance(&trait_ref, span).await;

        // Every arguments are packaged into a single tuple, which is passed as the
        // second parameter to Def.call.
        let types = arguments.iter().map(|id| self.type_of_expression(*id)).collect::<Vec<_>>();
        let tuple_ty = Ty::new_tuple(self.engine().intern_unsized(types), self.engine());
        let tuple = self
            .insert_expression(
                TypedExprKind::Tuple(Tuple::new(arguments.clone())),
                syn.span(),
                tuple_ty,
            )
            .await;

        // Use the same signature substitution and effect introduction as
        // dictionary.call. `Def.call` has no type parameter of its own.
        let trait_def_id = self.engine().get_core_item(CoreItem::DefCall).await;

        self.build_resolved_direct_call(
            ResolvedCallTarget::UnresolvedInstanceAssociated { instance, trait_ref, trait_def_id },
            vec![callee, tuple],
            Subst::new_empty(),
            span,
            Some(&arguments),
        )
        .await
    }
}

impl Bind<DirectCallSyn> for TAstBuilder {
    async fn bind(&mut self, syn: DirectCallSyn) -> TypedExprID {
        self.build_path_direct_call(&syn).await
    }
}
