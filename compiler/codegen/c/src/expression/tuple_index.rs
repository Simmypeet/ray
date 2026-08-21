use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, tuple_index::TupleIndex};

use crate::{context::Context, expr_ctx::ExprCtx, expression::Generate, writer::Writer};

impl Generate<TupleIndex> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &TupleIndex,
        _: TypedExprID,
        _: &ExprCtx,
        _: &mut Context,
    ) -> std::io::Result<()> {
        write!(self, "elem{:X}", expr.index())
    }
}
