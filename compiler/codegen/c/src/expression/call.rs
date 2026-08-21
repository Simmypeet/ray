use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, call::Call};

use crate::{
    context::Context,
    expr_ctx::ExprCtx,
    expression::Generate,
    writer::{EnclosingPair, Writer},
};

impl Generate<Call> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Call,
        _: TypedExprID,
        expr_ctx: &ExprCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let name = ctx.get_def_name(expr.function_id()).await;
        write!(self, "ray_{}", &*name)?;

        Box::pin(self.write_separated_list(
            EnclosingPair::Parens,
            expr.arguments().iter().copied(),
            ", ",
            async |writer, argument| writer.generate_typed_expr(argument, expr_ctx, ctx).await,
        ))
        .await
    }
}
