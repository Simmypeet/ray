use rayc_ir::ir_expr::{IRExpr, IRExprKind, load::Load};
use rayc_type::capture::LoadKind;
use rayc_typed_ast::typed_expr::r#move::Move;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Move>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Move>,
    ) -> LoweredExpression {
        match self.lower_by_id(context, expression.node().operand()).await {
            // Force the load to consume the place even when its type is `Copy`.
            LoweredExpression::LValue(address) => {
                let typed_expression = context.expression(expression.id());
                LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                    IRExprKind::Load(Load::with_kind(address, LoadKind::Move)),
                    typed_expression.span(),
                    typed_expression.ty().clone(),
                )))
            }

            // A computed value is a fresh temporary, so it already moves.
            LoweredExpression::RValue(value) => LoweredExpression::RValue(value),
        }
    }
}
