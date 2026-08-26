use rayc_source_file::SourceElement;
use rayc_syntax::{statement::Statement as StatementSyntax, r#type::Type as TypeSyntax};
use rayc_type::poly_var::get_enclosing_poly_var_maps;
use rayc_typed_ast::{
    name_binding::Source,
    statement::{Let, Return, Statement},
    typed_function::TypedFunctionLocalID,
    typed_variable::TypedVariable,
};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl TAstBuilder {
    async fn resolve_local_type_annotation(
        &mut self,
        syntax: &TypeSyntax,
    ) -> qbice::storage::intern::Interned<rayc_type::ty::Ty> {
        let poly_vars = self.engine().get_enclosing_poly_var_maps(self.current_def_id()).await;
        let resolution =
            rayc_resolution::resolve_type_with_poly_vars(self.engine(), &poly_vars, syntax);

        let ty = resolution.ty().clone();
        for diagnostic in resolution.into_diagnostics() {
            self.push_diagnostic(crate::diagnostic::Diagnostic::Resolution(diagnostic));
        }
        ty
    }

    pub async fn bind_statement(&mut self, statement: &StatementSyntax) {
        match statement {
            StatementSyntax::Let(l) => {
                let Some(expr) = l.expression() else {
                    return;
                };

                let pattern = l.pattern();

                let expr_id = self.bind(expr).await;

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

                self.push_variable_assignment_constraint(&var_ty, expr_id);

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
