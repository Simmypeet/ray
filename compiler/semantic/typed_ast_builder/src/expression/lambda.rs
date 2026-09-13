use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_syntax::{
    expression::{Expression, NLambda as NLambdaSyntax},
    irrefutable_pattern::IrrefutablePattern,
};
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    ty::{Ty, application::Closure},
};
use rayc_typed_ast::{
    name_binding::Source,
    statement::{Return, Statement},
    typed_expr::{TypedExprID, TypedExprKind, nlambda::NLambda as TypedNLambda},
    typed_function::TypedFunctionLocalID,
    typed_lambda::TypedLambdaParameter,
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<NLambdaSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: NLambdaSyntax) -> TypedExprID {
        let parameters =
            syn.parameter_list().map(|list| list.parameters().collect()).unwrap_or_default();
        self.bind_lambda(parameters, syn.body(), syn.span()).await
    }
}

impl TAstBuilder {
    async fn bind_lambda(
        &mut self,
        parameters: Vec<IrrefutablePattern>,
        body: Option<Expression>,
        span: RelativeSpan,
    ) -> TypedExprID {
        let function_id = self.start_lambda();
        let parameter_name_binding_group = self.parameter_name_binding_group();
        let mut parameter_types = Vec::new();

        for parameter_pattern in parameters {
            let ty = self.new_type_inference();
            let parameter_id = self.insert_lambda_parameter(TypedLambdaParameter::new(
                ty.clone(),
                parameter_pattern.span(),
            ));

            self.insert_name_binding_to_group_from_pattern(
                parameter_name_binding_group,
                &parameter_pattern,
                &ty,
                Source::LambdaParameter(TypedFunctionLocalID::new(function_id, parameter_id)),
            );
            parameter_types.push(ty);
        }

        let body = if let Some(body) = body {
            Box::pin(self.bind(body)).await
        } else {
            self.push_error_expression(span).await
        };
        let return_type = self.type_of_expression(body);
        self.push_statement(Statement::Return(Return::new_with_value(body))).await;

        let effect_row = self.finish_lambda();
        let (kind, ty) = {
            // The capture pass resolves this placeholder once all nested bodies exist.
            let captures = self.defer_closure_captures(function_id, span);
            let closure_id = self.register_closure(function_id);
            let owner_id = self.current_def_id();
            let poly_vars = self.engine().get_enclosing_poly_var_maps(owner_id).await;
            let owner_arguments = poly_vars
                .all_poly_vars()
                .map(|id| self.engine().intern(Ty::PolyVar(id)))
                .collect::<Vec<_>>();
            (
                TypedExprKind::NLambda(TypedNLambda::new(function_id)),
                Ty::new_closure(
                    Closure::new(owner_id, closure_id, owner_arguments.len()),
                    owner_arguments,
                    parameter_types,
                    return_type,
                    effect_row,
                    captures,
                    self.engine(),
                ),
            )
        };
        self.insert_expression(kind, span, ty).await
    }
}
