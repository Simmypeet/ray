use rayc_source_file::SourceElement;
use rayc_syntax::expression::IfElse as IfElseSyntax;
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, if_else::IfElse};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<IfElseSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: IfElseSyntax) -> TypedExprID {
        let condition = if let Some(condition) = syn.condition().and_then(|x| x.expression()) {
            Some(Box::pin(self.bind(condition)).await)
        } else {
            None
        };
        let then_expression = if let Some(expression) = syn.then_arm().and_then(|x| x.expression())
        {
            Some(Box::pin(self.bind(expression)).await)
        } else {
            None
        };
        let else_expression = if let Some(expression) = syn.else_arm().and_then(|x| x.expression())
        {
            Some(Box::pin(self.bind(expression)).await)
        } else {
            None
        };

        let (Some(condition), Some(then_expression), Some(else_expression)) =
            (condition, then_expression, else_expression)
        else {
            let children =
                [condition, then_expression, else_expression].into_iter().flatten().collect();
            return self.push_error_expression_with_children(syn.span(), children).await;
        };

        let bool_ty = Ty::new_primitive(Primitive::Bool, self.engine());
        self.push_if_condition_constraint(&bool_ty, condition).await;

        let ty = self.new_type_inference();
        self.push_if_branch_constraint(&ty, then_expression).await;
        self.push_if_branch_constraint(&ty, else_expression).await;

        self.insert_expression(
            TypedExprKind::IfElse(IfElse::new(condition, then_expression, else_expression)),
            syn.span(),
            ty,
        )
        .await
    }
}
