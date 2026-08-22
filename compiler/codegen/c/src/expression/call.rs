use std::io::Write;

use rayc_ir::{expression::call::Call, function::Function};

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Call> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Call>,
        _function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let call = expression.node();
        let name = ctx.get_def_name(call.function_id()).await;
        write!(self, "ray_{}(", &*name)?;
        for (index, argument) in call.arguments().iter().enumerate() {
            if index != 0 {
                write!(self, ", ")?;
            }
            write!(self, "ray_expr_{:X}", argument.index())?;
        }
        write!(self, ")")
    }
}
