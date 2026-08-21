use rayc_typed_ast::typed_expr::{TypedExprID, paren::Paren};

use crate::{
    context::Context,
    expr_ctx::ExprCtx,
    expression::Generate,
    writer::{EnclosingPair, Writer},
};

impl Generate<Paren> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Paren,
        _: TypedExprID,
        expr_ctx: &ExprCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            writer.generate_typed_expr(expr.expression(), expr_ctx, ctx).await
        }))
        .await
    }
}
