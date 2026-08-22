use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, deref::Deref};

use crate::{
    context::Context,
    expression::Generate,
    function_ctx::FunctionCtx,
    writer::{EnclosingPair, Writer},
};

impl Generate<Deref> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Deref,
        _: TypedExprID,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            write!(writer, "*")?;
            writer
                .write_enclosing_pair(EnclosingPair::Parens, async |writer| {
                    writer.generate_typed_expr(expr.pointee(), function_ctx, ctx).await
                })
                .await
        }))
        .await
    }
}
