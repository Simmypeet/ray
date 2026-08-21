use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, ref_of::RefOf};

use crate::{
    context::Context,
    expr_ctx::ExprCtx,
    expression::Generate,
    writer::{EnclosingPair, Writer},
};

impl Generate<RefOf> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &RefOf,
        _: TypedExprID,
        expr_ctx: &ExprCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            write!(writer, "&")?;
            writer
                .write_enclosing_pair(EnclosingPair::Parens, async |writer| {
                    writer.generate_typed_expr(expr.pointee(), expr_ctx, ctx).await
                })
                .await
        }))
        .await
    }
}
