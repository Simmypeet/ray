use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind};

use crate::{context::Context, expr_ctx::ExprCtx, writer::Writer};

pub mod binary;
pub mod call;
pub mod deref;
pub mod identifier;
pub mod literal;
pub mod paren;
pub mod ref_of;
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
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let expr = expr_ctx.get_typed_expr(typed_expr_id);

        match expr.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.generate(identifier, typed_expr_id, expr_ctx, ctx).await
            }
            TypedExprKind::Literal(literal) => {
                self.generate(literal, typed_expr_id, expr_ctx, ctx).await
            }
            TypedExprKind::TupleIndex(tuple_index) => {
                self.generate(tuple_index, typed_expr_id, expr_ctx, ctx).await
            }
            TypedExprKind::Tuple(tuple) => self.generate(tuple, typed_expr_id, expr_ctx, ctx).await,
            TypedExprKind::Call(call) => self.generate(call, typed_expr_id, expr_ctx, ctx).await,
            TypedExprKind::Binary(binary) => {
                self.generate(binary, typed_expr_id, expr_ctx, ctx).await
            }
            TypedExprKind::RefOf(ref_of) => {
                self.generate(ref_of, typed_expr_id, expr_ctx, ctx).await
            }
            TypedExprKind::Deref(deref) => self.generate(deref, typed_expr_id, expr_ctx, ctx).await,
            TypedExprKind::Paren(paren) => self.generate(paren, typed_expr_id, expr_ctx, ctx).await,
            TypedExprKind::Errored(_) => {
                panic!("errored expression reached codegen, this should have been caught earlier")
            }
        }
    }
}
