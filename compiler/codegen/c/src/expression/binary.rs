use std::io::Write;

use rayc_typed_ast::typed_expr::{
    TypedExprID,
    binary::{Binary, BinaryOp},
};

use crate::{
    context::Context,
    expr_ctx::ExprCtx,
    expression::Generate,
    writer::{EnclosingPair, Writer},
};

impl Generate<Binary> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Binary,
        _: TypedExprID,
        expr_ctx: &ExprCtx,
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
            writer.generate_typed_expr(expr.left(), expr_ctx, ctx).await?;
            write!(writer, "{operator}")?;
            writer.generate_typed_expr(expr.right(), expr_ctx, ctx).await
        }))
        .await
    }
}
