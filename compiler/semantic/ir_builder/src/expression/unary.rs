use rayc_ir::ir_expr::{
    IRExpr, IRExprKind,
    unary::{Unary as IrUnary, UnaryOp as IrUnaryOp},
};
use rayc_typed_ast::typed_expr::unary::{Unary, UnaryOp};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Unary>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Unary>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let unary = expression.node();
        let operator = match unary.operator() {
            UnaryOp::Negate => IrUnaryOp::Negate,
        };

        let operand = self.lower_rvalue_by_id(context, unary.operand()).await;
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Unary(IrUnary::new(operator, operand)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
