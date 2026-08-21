use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind};

use crate::{context::Context, expr_ctx::ExprCtx, writer::Writer};

pub mod identifier;
pub mod literal;
pub mod tuple;
pub mod tuple_index;

pub trait Generate<T> {
    #[expect(async_fn_in_trait)]
    async fn generate(
        &mut self,
        expr: &T,
        typed_expr_id: TypedExprID,
        expr_ctx: &ExprCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()>;
}

impl Writer<'_> {
    pub async fn generate_typed_expr(
        &mut self,
        typed_expr_id: TypedExprID,
        expr_ctx: &ExprCtx,
        generator: &mut Context,
    ) -> std::io::Result<()> {
        let expr = expr_ctx.get_typed_expr(typed_expr_id);

        match expr.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.generate(identifier, typed_expr_id, expr_ctx, generator).await
            }
            TypedExprKind::Literal(literal) => {
                self.generate(literal, typed_expr_id, expr_ctx, generator).await
            }
            TypedExprKind::TupleIndex(tuple_index) => {
                self.generate(tuple_index, typed_expr_id, expr_ctx, generator).await
            }
            TypedExprKind::Tuple(_) => todo!(),
            TypedExprKind::Call(_) => todo!(),
            TypedExprKind::Binary(_) => todo!(),
            TypedExprKind::RefOf(_) => todo!(),
            TypedExprKind::Deref(_) => todo!(),
            TypedExprKind::Paren(_) => todo!(),
            TypedExprKind::Errored(_) => todo!(),
        }
    }
}
