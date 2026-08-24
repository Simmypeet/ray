use std::io::Write;

use rayc_ir::{expression::literal::Literal, function::Function};
use rayc_mono::MonoFunction;

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Literal> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Literal>,
        _function: &Function,
        _mono_function: &MonoFunction,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        match expression.node() {
            Literal::Numeric(value) => write!(self, "{value}"),
            Literal::Bool(true) => write!(self, "true"),
            Literal::Bool(false) => write!(self, "false"),
        }
    }
}
