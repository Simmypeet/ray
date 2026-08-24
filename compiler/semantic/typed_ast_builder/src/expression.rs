use rayc_syntax::expression::Expression;
use rayc_typed_ast::typed_expr::TypedExprID;

use crate::{bind::Bind, tast_builder::TAstBuilder};

pub mod binary;
pub mod call;
pub mod identifier;
pub mod if_else;
pub mod lambda;
pub mod leaf;
pub mod literal;
pub mod parenthesized;
pub mod postfix;
pub mod r#return;

impl Bind<Expression> for TAstBuilder {
    async fn bind(&mut self, syn: Expression) -> TypedExprID {
        match syn {
            Expression::IfElse(if_else) => Box::pin(self.bind(if_else)).await,
            Expression::Binary(binary) => Box::pin(self.bind(binary)).await,
            Expression::Lambda(lambda) => Box::pin(self.bind(lambda)).await,
        }
    }
}
