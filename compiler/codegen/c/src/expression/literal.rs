use std::io::Write;

use rayc_ir::ir_expr::literal::Literal;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Literal> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Literal>,
        _function: FunctionInstance<'_>,
        _ctx: &Context,
    ) -> std::io::Result<()> {
        match expression.node() {
            Literal::Numeric(value) => write!(self, "{value}"),
            Literal::Bool(true) => write!(self, "true"),
            Literal::Bool(false) => write!(self, "false"),
            Literal::String(value) => {
                write!(self, "\"")?;
                for byte in value.as_bytes() {
                    write!(self, "\\{byte:03o}")?;
                }
                write!(self, "\"")
            }
        }
    }
}
