use std::io::Write;

use rayc_ir::ir_expr::binary::{Binary, BinaryOp};

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Binary> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Binary>,
        _function: FunctionInstance<'_>,
        _ctx: &Context,
    ) -> std::io::Result<()> {
        let binary = expression.node();
        let operator = match binary.operator() {
            BinaryOp::Equal => "==",
            BinaryOp::NotEqual => "!=",
            BinaryOp::Plus => "+",
            BinaryOp::Minus => "-",
            BinaryOp::Multiply => "*",
            BinaryOp::Divide => "/",
        };
        write!(
            self,
            "({} {operator} {})",
            Identifier::expr(binary.left()),
            Identifier::expr(binary.right())
        )
    }
}
