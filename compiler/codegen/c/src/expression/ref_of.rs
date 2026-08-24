use std::io::Write;

use rayc_ir::expression::ref_of::RefOf;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&RefOf> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&RefOf>,
        _function: FunctionInstance<'_>,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        write!(self, "&(")?;
        self.write_address(expression.node().address())?;
        write!(self, ")")
    }
}
