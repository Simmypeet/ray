use std::io::Write;

use rayc_ir::ir_expr::ref_of::RefOf;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&RefOf> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&RefOf>,
        function: FunctionInstance<'_>,
        _ctx: &Context,
    ) -> std::io::Result<()> {
        write!(self, "&(")?;
        self.write_address(expression.node().address(), function)?;
        write!(self, ")")
    }
}
