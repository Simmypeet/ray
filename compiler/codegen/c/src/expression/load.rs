use rayc_ir::ir_expr::load::Load;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Load> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Load>,
        function: FunctionInstance<'_>,
        _ctx: &Context,
    ) -> std::io::Result<()> {
        self.write_address(expression.node().address(), function)
    }
}
