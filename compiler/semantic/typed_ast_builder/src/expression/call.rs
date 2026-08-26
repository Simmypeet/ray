use qbice::storage::intern::Interned;
use rayc_lexical::tree::RelativeSpan;
use rayc_semantic_element::{parameter::get_parameter_map, return_type::get_return_type};
use rayc_source_file::SourceElement;
use rayc_symbol::{GlobalSymbolID, syntax::is_variadic_def};
use rayc_syntax::{Identifier, expression::Call as CallSyn};
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::{Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView},
};
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, call::Call};

use crate::{
    bind::Bind,
    diagnostic::{
        Diagnostic, ExpectedLambdaType, MismatchedArgumentCount, MismatchedIndirectArgumentCount,
    },
    tast_builder::TAstBuilder,
};

enum LambdaCallSignature {
    Callable { parameter_types: Vec<Interned<Ty>>, return_type: Interned<Ty> },
    Invalid,
}

impl TAstBuilder {
    async fn instantiate_poly_vars(&mut self, function_id: GlobalSymbolID) -> Subst {
        let poly_var_stack = self.engine().get_enclosing_poly_var_maps(function_id).await;
        poly_var_stack
            .all_poly_vars_with_kind()
            .map(|(global_poly_var_id, kind)| {
                (global_poly_var_id, self.new_type_inference_with_kind(kind))
            })
            .collect()
    }

    async fn bind_call_arguments(&mut self, syn: &CallSyn) -> Vec<TypedExprID> {
        let mut arguments = Vec::new();

        if let Some(argument_list) = syn.arguments() {
            for argument in argument_list.expressions() {
                arguments.push(self.bind(argument).await);
            }
        }

        arguments
    }

    pub async fn build_bare_identifier_call(
        &mut self,
        identifier: Identifier,
        syn: &CallSyn,
    ) -> TypedExprID {
        if self.lookup_name_binding(&identifier.kind.0).is_some() {
            let callee = self.bind(identifier).await;
            return self.build_lambda_call(callee, syn).await;
        }

        self.build_direct_call(identifier, syn).await
    }

    async fn build_direct_call(&mut self, identifier: Identifier, syn: &CallSyn) -> TypedExprID {
        let arguments = self.bind_call_arguments(syn).await;
        let span = identifier.span().join(&syn.span());

        let Some(function_id) = self.resolve_function_id(&identifier).await else {
            return self.push_error_expression_with_children(span, arguments);
        };

        let call_subst = self.instantiate_poly_vars(function_id).await;
        let parameter_map = self.engine().get_parameter_map(function_id).await;

        let is_variadic = self.engine().is_variadic_def(function_id).await;
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

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Call(Call::new_direct(function_id, arguments, call_subst)),
            span,
            return_type,
        ))
    }

    pub async fn build_lambda_call(&mut self, callee: TypedExprID, syn: &CallSyn) -> TypedExprID {
        let arguments = self.bind_call_arguments(syn).await;
        let callee_span = self.span_of_expression(callee);
        let span = callee_span.join(&syn.span());

        let return_type =
            match self.resolve_lambda_call_signature(callee, arguments.len(), callee_span) {
                LambdaCallSignature::Callable { parameter_types, return_type } => {
                    self.check_lambda_call_arguments(&parameter_types, &arguments, span);
                    return_type
                }
                LambdaCallSignature::Invalid => Ty::new_error(self.engine()),
            };

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Call(Call::new_lambda(callee, arguments)),
            span,
            return_type,
        ))
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
                let expected = Ty::new_lambda(
                    parameter_types.iter().cloned(),
                    return_type.clone(),
                    self.engine(),
                );
                self.push_lambda_invocation_constraint(&expected, callee);
                LambdaCallSignature::Callable { parameter_types, return_type }
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
