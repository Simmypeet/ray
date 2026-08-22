use std::io::Write;

use rayc_typed_ast::typed_expr::{TypedExprID, literal::Literal};

use crate::{context::Context, expression::Generate, function_ctx::FunctionCtx, writer::Writer};

impl Generate<Literal> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Literal,
        _: TypedExprID,
        _: &FunctionCtx,
        _: &mut Context,
    ) -> std::io::Result<()> {
        match expr {
            Literal::Numeric(num) => write!(self, "{num}"),
            Literal::Bool(true) => write!(self, "true"),
            Literal::Bool(false) => write!(self, "false"),
        }
    }
}
