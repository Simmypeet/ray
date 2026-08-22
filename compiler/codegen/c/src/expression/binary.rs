use std::io::Write;

use rayc_typed_ast::typed_expr::{
    TypedExprID,
    binary::{Binary, BinaryOp},
};

use crate::{
    context::Context,
    expression::Generate,
    function_ctx::FunctionCtx,
    writer::{EnclosingPair, Writer},
};

impl Generate<Binary> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Binary,
        _: TypedExprID,
        function_ctx: &FunctionCtx,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let operator = match expr.operator() {
            BinaryOp::Assign => " = ",
            BinaryOp::Plus => " + ",
            BinaryOp::Minus => " - ",
            BinaryOp::Multiply => " * ",
            BinaryOp::Divide => " / ",
            BinaryOp::And => " && ",
            BinaryOp::Or => " || ",
        };

        Box::pin(self.write_enclosing_pair(EnclosingPair::Parens, async |writer| {
            writer.generate_typed_expr(expr.left(), function_ctx, ctx).await?;
            write!(writer, "{operator}")?;
            writer.generate_typed_expr(expr.right(), function_ctx, ctx).await
        }))
        .await
    }
}
