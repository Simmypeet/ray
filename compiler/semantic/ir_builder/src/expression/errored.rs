use rayc_ir::ir_expr::IRExpr;
use rayc_typed_ast::typed_expr::errored::Errored;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Errored>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Errored>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        LoweredExpression::RValue(self.emit_expression(IRExpr::new_error(span, ty)))
    }
}
