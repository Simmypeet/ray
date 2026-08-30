use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_resolution::path::{Effect, PathResolution};
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, symbol_kind::SymbolKind, syntax::is_variadic_def};
use rayc_syntax::expression::{Call as CallSyn, DirectCall as DirectCallSyn};
use rayc_type::{
    subst::{Subst, Substitutable},
    ty::{Ty, TyKind, application::View as ApplicationView, effect_row::EffectLabel},
};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, call::Call};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, ExpectedLambdaType, MismatchedArgumentCount, MismatchedIndirectArgumentCount,
    },
    tast_builder::TAstBuilder,
};

enum LambdaCallSignature {
    Callable {
        parameter_types: Vec<Interned<Ty>>,
        return_type: Interned<Ty>,
        effect_row: Interned<Ty>,
    },
    Invalid,
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
            return self.push_error_expression(syn.span());
        };
        let Some(call) = syn.call() else {
            return self.push_error_expression(syn.span());
        };

        if let Some(identifier) = path.bare_identifier()
            && self.lookup_name_binding(&identifier.kind.0).is_some()
        {
            let callee = self.bind(identifier).await;
            return self.build_lambda_call(callee, &call).await;
        }

        let arguments = self.bind_call_arguments(&call).await;
        let Ok(resolution) = self.resolve_path(&path).await else {
            return self.push_error_expression_with_children(syn.span(), arguments);
        };
        let (function_id, symbol_kind, call_subst, operation_effect) = match &resolution {
            PathResolution::Def(def) => {
                (def.symbol_id(), SymbolKind::Def, def.substitution(self.engine()).await, None)
            }
            PathResolution::ExternDef(def) => {
                (def.symbol_id(), SymbolKind::ExternDef, Subst::new_empty(), None)
            }
            PathResolution::EffectOperation(operation) => (
                operation.symbol_id(),
                SymbolKind::EffectOperation,
                operation.substitution(self.engine()).await,
                Some(operation.effect()),
            ),
            PathResolution::Module(module) => {
                self.push_symbol_not_callable(module.symbol_id(), path.span());
                return self.push_error_expression_with_children(syn.span(), arguments);
            }
            PathResolution::Effect(effect) => {
                self.push_symbol_not_callable(effect.symbol_id(), path.span());
                return self.push_error_expression_with_children(syn.span(), arguments);
            }
        };
        self.build_resolved_direct_call(
            function_id,
            symbol_kind,
            operation_effect,
            arguments,
            call_subst,
            syn.span(),
        )
        .await
    }

    async fn build_resolved_direct_call(
        &mut self,
        function_id: GlobalSymbolID,
        symbol_kind: SymbolKind,
        operation_effect: Option<&Effect>,
        arguments: Vec<TypedExprID>,
        call_subst: Subst,
        span: RelativeSpan,
    ) -> TypedExprID {
        let parameter_map = self.engine().get_parameter_map(function_id).await;

        let is_variadic = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::ExternDef) {
            self.engine().is_variadic_def(function_id).await
        } else {
            false
        };
        if (!is_variadic && parameter_map.len() != arguments.len())
            || (is_variadic && arguments.len() < parameter_map.len())
        {
            self.push_diagnostic(Diagnostic::MismatchedArgumentCount(
                MismatchedArgumentCount::builder()
                    .calling_symbol(function_id)
                    .expected(parameter_map.len())
                    .found(arguments.len())
                    .span(span)
                    .build(),
            ));
        }

        for ((_, parameter), argument) in parameter_map.iter().zip(arguments.iter()) {
            let parameter_ty = parameter.ty().apply_subst_or_clone(&call_subst, self.engine());
            self.push_function_call_constraint(&parameter_ty, *argument);
        }

        let return_type = self.engine().get_return_type(function_id).await;
        let return_type = return_type.apply_subst_or_clone(&call_subst, self.engine());

        let effect_row = if let Some(effect) = operation_effect {
            let label =
                self.engine().intern(EffectLabel::new(effect.symbol_id(), effect.args().clone()));
            Ty::new_effect_row([label], None, self.engine())
        } else {
            self.engine()
                .get_effect_row(function_id)
                .await
                .apply_subst_or_clone(&call_subst, self.engine())
        };

        let expr_id = self.insert_expression(
            TypedExprKind::Call(Call::new_direct(function_id, arguments, call_subst)),
            span,
            return_type,
        );
        self.push_effect_introduction(expr_id, &effect_row);
        expr_id
    }

    fn push_symbol_not_callable(&mut self, symbol_id: GlobalSymbolID, span: RelativeSpan) {
        self.push_diagnostic(Diagnostic::SymbolNotCallable(
            crate::diagnostic::SymbolNotCallable::builder().name(symbol_id).span(span).build(),
        ));
    }

    pub async fn build_lambda_call(&mut self, callee: TypedExprID, syn: &CallSyn) -> TypedExprID {
        let arguments = self.bind_call_arguments(syn).await;
        let callee_span = self.span_of_expression(callee);
        let span = callee_span.join(&syn.span());

        let (return_type, effect_row) =
            match self.resolve_lambda_call_signature(callee, arguments.len(), callee_span) {
                LambdaCallSignature::Callable { parameter_types, return_type, effect_row } => {
                    self.check_lambda_call_arguments(&parameter_types, &arguments, span);

                    (return_type, Some(effect_row))
                }
                LambdaCallSignature::Invalid => (Ty::new_star_error(self.engine()), None),
            };

        let expr_id = self.insert_expression(
            TypedExprKind::Call(Call::new_lambda(callee, arguments)),
            span,
            return_type,
        );

        // if the lambda effect signature is malformed, don't bother adding the effect
        // introduction constraint, as it will just add noise to the diagnostics
        if let Some(effect_row) = effect_row {
            self.push_effect_introduction(expr_id, &effect_row);
        }

        expr_id
    }

    fn resolve_lambda_call_signature(
        &mut self,
        callee: TypedExprID,
        argument_count: usize,
        callee_span: RelativeSpan,
    ) -> LambdaCallSignature {
        let callee_ty = self.latest_type(&self.type_of_expression(callee));

        match &*callee_ty {
            Ty::Application(application) => match application.view() {
                ApplicationView::Lambda(lambda) => LambdaCallSignature::Callable {
                    parameter_types: lambda.parameter_types().to_vec(),
                    return_type: lambda.return_type().clone(),
                    effect_row: lambda.effect_row().clone(),
                },
                ApplicationView::Error => LambdaCallSignature::Invalid,
                ApplicationView::Primitive(_)
                | ApplicationView::Tuple(_)
                | ApplicationView::Pointer(_) => {
                    self.report_expected_lambda(callee_ty, callee_span);
                    LambdaCallSignature::Invalid
                }
            },
            Ty::Inference(_) => {
                let parameter_types =
                    (0..argument_count).map(|_| self.new_type_inference()).collect::<Vec<_>>();

                let return_type = self.new_type_inference();
                let effect_row = self.new_type_inference_with_kind(TyKind::EffectRow);
                let expected = Ty::new_lambda(
                    parameter_types.iter().cloned(),
                    return_type.clone(),
                    effect_row.clone(),
                    self.engine(),
                );
                self.push_lambda_invocation_constraint(&expected, callee);

                LambdaCallSignature::Callable { parameter_types, return_type, effect_row }
            }
            Ty::PolyVar(_) => {
                self.report_expected_lambda(callee_ty, callee_span);
                LambdaCallSignature::Invalid
            }
            Ty::EffectRow(_) => {
                todo!("resolve a lambda call whose callee has an effect-row type")
            }
        }
    }

    fn report_expected_lambda(&mut self, ty: Interned<Ty>, span: RelativeSpan) {
        self.push_diagnostic(Diagnostic::ExpectedLambdaType(
            ExpectedLambdaType::builder().ty(ty).span(span).build(),
        ));
    }

    fn check_lambda_call_arguments(
        &mut self,
        parameter_types: &[Interned<Ty>],
        arguments: &[TypedExprID],
        call_span: RelativeSpan,
    ) {
        if parameter_types.len() != arguments.len() {
            self.push_diagnostic(Diagnostic::MismatchedIndirectArgumentCount(
                MismatchedIndirectArgumentCount::builder()
                    .expected(parameter_types.len())
                    .found(arguments.len())
                    .span(call_span)
                    .build(),
            ));
        }

        for (parameter_type, argument) in parameter_types.iter().zip(arguments.iter()) {
            self.push_lambda_invocation_constraint(parameter_type, *argument);
        }
    }
}

impl Bind<DirectCallSyn> for TAstBuilder {
    async fn bind(&mut self, syn: DirectCallSyn) -> TypedExprID {
        self.build_path_direct_call(&syn).await
    }
}
