use rayc_source_file::SourceElement;
use rayc_syntax::statement::Statement as StatementSyntax;
use rayc_typed_ast::{
    typed_function::FunctionLocalID,
    name_binding::Source,
    statement::{Let, Return, Statement},
    variable::Variable,
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl TAstBuilder {
    pub async fn bind_statement(&mut self, statement: &StatementSyntax) {
        match statement {
            StatementSyntax::Let(l) => {
                let Some(expr) = l.expression() else {
                    return;
                };

                let pattern = l.pattern();

                let expr_id = self.bind(expr).await;

                let var_ty = self.new_type_inference();
                let var_id = self.insert_variable(Variable::new(
                    var_ty.clone(),
                    pattern.as_ref().map_or_else(|| l.span(), SourceElement::span),
                ));

                self.push_variable_assignment_constraint(&var_ty, expr_id);

                let name_binding_group_id = self.push_new_name_binding_group();

                if let Some(pat) = pattern {
                    self.insert_name_binding_to_group_from_pattern(
                        name_binding_group_id,
                        &pat,
                        &var_ty,
                        Source::Variable(FunctionLocalID::new(
                            self.current_typed_function_id(),
                            var_id,
                        )),
                    );
                }

                self.push_statement(Statement::Let(
                    Let::builder()
                        .variable_id(var_id)
                        .name_binding_group_id(name_binding_group_id)
                        .expression(expr_id)
                        .span(l.span())
                        .build(),
                ));
            }

            StatementSyntax::Expression(expression) => {
                let expr = self.bind(expression.clone()).await;

                self.push_statement(Statement::Expression(expr));
            }

            StatementSyntax::Return(ret) => {
                let ret = if let Some(expression) = ret.expression() {
                    let expression = self.bind(expression).await;
                    self.push_return_type_constraint(expression).await;

                    Return::new_with_value(expression)
                } else {
                    self.push_unit_return_type_constraint(ret.span()).await;

                    Return::new_unit()
                };

                self.push_statement(Statement::Return(ret));
            }
        }
    }
}
