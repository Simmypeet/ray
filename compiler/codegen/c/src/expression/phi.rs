use rayc_ir::expression::phi::Phi;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Phi> for Writer<'_> {
    async fn write_expression(
        &mut self,
        _expression: ExpressionWithID<&Phi>,
        _function: FunctionInstance<'_>,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        panic!("phi expression cannot be emitted as an ordinary C expression")
    }
}
