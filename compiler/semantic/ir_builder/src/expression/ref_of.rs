use rayc_ir::ir_expr::{ExpressionID, IRExpr, IRExprKind, ref_of::RefOf as IrRefOf};
use rayc_typed_ast::typed_expr::ref_of::RefOf;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a RefOf>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RefOf>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let reference = expression.node();
        let address = self.lower_address_by_id(context, reference.pointee());
        self.emit_expression(IRExpr::new(IRExprKind::RefOf(IrRefOf::new(address)), span, ty))
    }
}
