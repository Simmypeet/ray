use rayc_source_file::SourceElement;
use rayc_syntax::expression::Lambda as LambdaSyntax;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::FunctionLocalID,
    lambda::LambdaParameter,
    name_binding::Source,
    statement::{Return, Statement},
    typed_expr::{TypedExpr, TypedExprID, TypedExprKind, lambda::Lambda as TypedLambda},
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<LambdaSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: LambdaSyntax) -> TypedExprID {
        let span = syn.span();
        let function_id = self.start_lambda();
        let parameter_name_binding_group = self.parameter_name_binding_group();
        let mut parameter_types = Vec::new();

        if let Some(parameters) = syn.parameters() {
            for parameter_pattern in parameters.parameters() {
                let ty = self.new_type_inference();
                let parameter_id = self.insert_lambda_parameter(LambdaParameter::new(
                    ty.clone(),
                    parameter_pattern.span(),
                ));

                self.insert_name_binding_to_group_from_pattern(
                    parameter_name_binding_group,
                    &parameter_pattern,
                    &ty,
                    Source::LambdaParameter(FunctionLocalID::new(function_id, parameter_id)),
                );
                parameter_types.push(ty);
            }
        }

        let body = if let Some(body) = syn.body() {
            Box::pin(self.bind(body)).await
        } else {
            self.push_error_expression(span)
        };
        let return_type = self.type_of_expression(body);
        self.push_statement(Statement::Return(Return::new_with_value(body)));

        self.finish_lambda();

        let ty = Ty::new_lambda(parameter_types, return_type, self.engine());
        self.insert_expression(TypedExpr::new(
            TypedExprKind::Lambda(TypedLambda::new(function_id)),
            span,
            ty,
        ))
    }
}
