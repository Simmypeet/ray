use rayc_syntax::expression::Leaf;
use rayc_typed_ast::typed_expr::TypedExprID;

use crate::{bind::Bind, tast_builder::TAstBuilder};

impl Bind<Leaf> for TAstBuilder {
    async fn bind(&mut self, syn: Leaf) -> TypedExprID {
        match syn {
            Leaf::Call(call) => self.bind(call).await,
            Leaf::Identifier(token) => self.bind(token).await,
            Leaf::Literal(literal) => self.bind(literal).await,
            Leaf::Parenthesized(parenthesized) => self.bind(parenthesized).await,
        }
    }
}
