use rayc_ir::ir_expr::{IRExpr, IRExprKind, ref_of::RefOf as IrRefOf};
use rayc_typed_ast::typed_expr::ref_of::RefOf;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a RefOf>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RefOf>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let reference = expression.node();
        let address = self.lower_lvalue_by_id(context, reference.pointee());
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::RefOf(IrRefOf::new(address)),
            span,
            ty,
        )))
    }
}
