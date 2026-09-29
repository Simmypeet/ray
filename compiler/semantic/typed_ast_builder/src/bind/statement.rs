use rayc_source_file::SourceElement;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_syntax::statement::{Return as ReturnSyntax, Statement as StatementSyntax};
use rayc_type::{
    trait_ref::TraitRef,
    ty::{Ty, TyKind, args::Args},
};
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
                let pattern = l.pattern();

                // The parser has already reported a malformed initializer, so it
                // is bound as an errored expression of the error type.
                let mut expr_id = match l.assignment().map(|a| (a.expression(), a.span())) {
                    Some((Some(expr), _)) => Some(self.bind(expr).await),
                    Some((None, span)) => Some(self.push_syntax_error_expression(span).await),
                    None => None,
                };

                // Only an annotated `let` is a coercion site. The parser has
                // already reported a malformed annotation, so its type is the
                // error type rather than an inference.
                let var_ty = match l.type_annotation().map(|a| a.r#type()) {
                    Some(Some(annotation)) => {
                        let var_ty = self.resolve_local_type_annotation(&annotation).await;
                        if let Some(id) = expr_id {
                            expr_id = Some(self.coerce(id, &var_ty).await);
                        }
                        var_ty
                    }
                    Some(None) => Ty::new_error(TyKind::Star, self.engine()),
                    None => self.new_type_inference(),
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

            StatementSyntax::Return(ret) => self.bind_return(ret).await,
        }
    }

    async fn bind_return(&mut self, ret: &ReturnSyntax) {
        let ret = if let Some(expression) = ret.expression() {
            // A returned value is a coercion site.
            let expression = self.bind(expression).await;
            let return_type = self.return_type_of_current_function().await;
            let expression = self.coerce(expression, &return_type).await;
            self.push_return_type_constraint(expression).await;

            Return::new_with_value(expression)
        } else {
            self.push_unit_return_type_constraint(ret.span()).await;

            Return::new_unit()
        };

        self.push_statement(Statement::Return(ret)).await;
    }
}
