use std::io::Write;

use rayc_ir::{
    expression::binary::{Binary, BinaryOp},
    function::Function,
};

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

impl WriteExpression<&Binary> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Binary>,
        _function: &Function,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        let binary = expression.node();
        let operator = match binary.operator() {
            BinaryOp::Plus => "+",
            BinaryOp::Minus => "-",
            BinaryOp::Multiply => "*",
            BinaryOp::Divide => "/",
        };
        write!(
            self,
            "(ray_expr_{:X} {operator} ray_expr_{:X})",
            binary.left().index(),
            binary.right().index()
        )
    }
}
