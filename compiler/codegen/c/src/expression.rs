use rayc_typed_ast::function::Function;

use crate::{context::Context, expr_ctx::ExprCtx};

pub mod identifier;
pub mod literal;

pub trait Generate<T> {
    #[expect(async_fn_in_trait)]
    async fn generate(
        &mut self,
        expr: &T,
        expr_ctx: &ExprCtx,
        generator: &mut Context,
    ) -> std::io::Result<()>;
}
