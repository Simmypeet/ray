use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, tuple_index::TupleIndex};

use crate::{
    context::Context,
    expression::Generate,
    function_ctx::FunctionCtx,
    writer::{EnclosingPair, Writer},
};

impl Generate<TupleIndex> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &TupleIndex,
        _: TypedExprID,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            writer.generate_typed_expr(expr.operand(), function_ctx, ctx).await
        }))
        .await?;

        write!(self, ".elem{:X}", expr.index())
    }
}
