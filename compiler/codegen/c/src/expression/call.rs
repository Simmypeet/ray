use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, call::Call};

use crate::{
    context::Context,
    expression::Generate,
    function_ctx::FunctionCtx,
    writer::{EnclosingPair, Writer},
};

impl Generate<Call> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Call,
        _: TypedExprID,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let name = ctx.get_def_name(expr.function_id()).await;
        write!(self, "ray_{}", &*name)?;

        Box::pin(self.write_separated_list(
            EnclosingPair::Parens,
            expr.arguments().iter().copied(),
            ", ",
            async |writer, argument| writer.generate_typed_expr(argument, function_ctx, ctx).await,
        ))
        .await
    }
}
