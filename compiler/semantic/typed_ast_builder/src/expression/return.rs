use rayc_source_file::SourceElement;
use rayc_syntax::expression::Return as ReturnSyntax;
use rayc_typed_ast::typed_expr::{TypedExpr, TypedExprID, TypedExprKind, r#return::Return};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<ReturnSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: ReturnSyntax) -> TypedExprID {
        let Some(inner_expr) = syn.expression() else {
            return self.push_error_expression(syn.span());
        };

        let inner_expr_id = self.bind(inner_expr).await;
        self.push_return_type_constraint(inner_expr_id).await;

        let infer_type = self.new_type_inference();

        self.insert_expression(TypedExpr::new(
            TypedExprKind::Return(Return::new(inner_expr_id)),
            syn.span(),
            infer_type,
        ))
    }
}
