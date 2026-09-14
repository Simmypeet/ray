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
pub mod run_with;
pub mod while_loop;

impl Bind<Expression> for TAstBuilder {
    async fn bind(&mut self, syn: Expression) -> TypedExprID {
        match syn {
            Expression::RunWith(run_with) => Box::pin(self.bind(run_with)).await,
            Expression::Closure(lambda) => Box::pin(self.bind(lambda)).await,
            Expression::IfElse(if_else) => Box::pin(self.bind(if_else)).await,
            Expression::While(while_loop) => Box::pin(self.bind(while_loop)).await,
            Expression::Binary(binary) => Box::pin(self.bind(binary)).await,
        }
    }
}
