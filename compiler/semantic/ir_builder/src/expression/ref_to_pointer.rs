use rayc_ir::ir_expr::{IRExpr, IRExprKind, ref_to_pointer::RefToPointer as IrRefToPointer};
use rayc_typed_ast::typed_expr::ref_to_pointer::RefToPointer;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a RefToPointer>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RefToPointer>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let reference = self.lower_rvalue_by_id(context, expression.node().reference()).await;
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::RefToPointer(IrRefToPointer::new(reference)),
            span,
            ty,
        )))
    }
}
