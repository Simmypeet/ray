use rayc_ir::{expression::load::Load, function::Function};

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Load> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Load>,
        _function: &Function,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        self.write_address(expression.node().address())
    }
}
