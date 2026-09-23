use rayc_source_file::SourceElement;
use rayc_syntax::expression::Move as MoveSyntax;
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind, r#move::Move};

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<MoveSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: MoveSyntax) -> TypedExprID {
        let Some(operand) = syn.operand() else {
            return self.push_error_expression(syn.span()).await;
        };

        // The moved value keeps the operand's type. The operand may itself
        // contain a `move`, so the recursion is boxed.
        let operand = Box::pin(self.bind(operand)).await;
        self.insert_expression(
            TypedExprKind::Move(Move::new(operand)),
            syn.span(),
            self.type_of_expression(operand),
        )
        .await
    }
}
