use rayc_source_file::SourceElement;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_syntax::statement::Statement as StatementSyntax;
use rayc_type::{trait_ref::TraitRef, ty::args::Args};
use rayc_typed_ast::{
    name_binding::Source,
    statement::{Break, Continue, ExpressionStatement, Let, Return, Statement},
    typed_function::TypedFunctionLocalID,
    typed_variable::TypedVariable,
};

use crate::{
    bind::Bind,
    diagnostic::{BreakOutsideLoop, ContinueOutsideLoop, Diagnostic},
    tast_builder::TAstBuilder,
};

impl TAstBuilder {
    pub async fn bind_statement(&mut self, statement: &StatementSyntax) {
        match statement {
            StatementSyntax::Let(l) => {
                let expr = l.assignment().and_then(|x| x.expression());
                let pattern = l.pattern();

                let expr_id =
                    if let Some(expr) = expr { Some(self.bind(expr).await) } else { None };

                let var_ty = if let Some(annotation) = l.type_annotation().and_then(|a| a.r#type())
                {
                    self.resolve_local_type_annotation(&annotation).await
                } else {
                    self.new_type_inference()
                };
                let var_id = self.insert_variable(TypedVariable::new(
                    var_ty.clone(),
                    pattern.as_ref().map_or_else(|| l.span(), SourceElement::span),
                ));

                if let Some(expr_id) = expr_id {
                    self.push_variable_assignment_constraint(&var_ty, expr_id).await;
                }

                let name_binding_group_id = self.push_new_name_binding_group();

                if let Some(pat) = pattern {
                    self.insert_name_binding_to_group_from_pattern(
                        name_binding_group_id,
                        &pat,
                        &var_ty,
                        Source::Variable(TypedFunctionLocalID::new(
                            self.current_typed_function_id(),
                            var_id,
                        )),
                    );
                }

                self.push_statement(Statement::Let(
                    Let::builder()
                        .variable_id(var_id)
                        .name_binding_group_id(name_binding_group_id)
                        .maybe_expression(expr_id)
                        .span(l.span())
                        .build(),
                ))
                .await;
            }

            StatementSyntax::Break(statement) => {
                let span = statement.span();
                if !self.is_inside_loop() {
                    self.push_diagnostic(Diagnostic::BreakOutsideLoop(BreakOutsideLoop::new(span)));
                }
                self.push_statement(Statement::Break(Break::new(span))).await;
            }

            StatementSyntax::Continue(statement) => {
                let span = statement.span();
                if !self.is_inside_loop() {
                    self.push_diagnostic(Diagnostic::ContinueOutsideLoop(
                        ContinueOutsideLoop::new(span),
                    ));
                }
                self.push_statement(Statement::Continue(Continue::new(span))).await;
            }

            StatementSyntax::Expression(expression) => {
                let expr = self.bind(expression.clone()).await;

                // The discarded value is dropped, so its type must have a Drop
                // dictionary in this context. The requirement is solved with
                // the other constraints, since the type may still be inferred.
                let drop_trait = self.engine().get_core_item(CoreItem::DropTrait).await;
                let trait_ref = TraitRef::new(
                    drop_trait,
                    Args::new([self.type_of_expression(expr)], self.engine()),
                );
                let drop_instance =
                    self.infer_trait_instance(&trait_ref, self.span_of_expression(expr)).await;

                self.push_statement(Statement::Expression(ExpressionStatement::new(
                    expr,
                    drop_instance,
                )))
                .await;
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

                self.push_statement(Statement::Return(ret)).await;
            }
        }
    }
}
