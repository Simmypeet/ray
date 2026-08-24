use rayc_ir::expression::load::Load;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Load> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Load>,
        _function: FunctionInstance<'_>,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        self.write_address(expression.node().address())
    }
}
