use rayc_ir::ir_expr::{IRExpr, IRExprKind, cast::Cast as IrCast};
use rayc_typed_ast::typed_expr::cast::Cast;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Cast>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Cast>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());

        let operand = self.lower_rvalue_by_id(context, expression.node().operand()).await;
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Cast(IrCast::new(operand)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
