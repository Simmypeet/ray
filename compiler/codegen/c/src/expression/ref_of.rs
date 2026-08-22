use std::io::Write;

use rayc_ir::{expression::ref_of::RefOf, function::Function};

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&RefOf> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&RefOf>,
        _function: &Function,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        write!(self, "&(")?;
        self.write_address(expression.node().address())?;
        write!(self, ")")
    }
}
