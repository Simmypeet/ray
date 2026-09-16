use rayc_ir::ir_expr::{IRExpr, IRExprKind, tuple::Tuple as IrTuple};
use rayc_typed_ast::typed_expr::tuple::Tuple;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Tuple>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Tuple>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let tuple = expression.node();
        let elements = tuple
            .elements()
            .iter()
            .map(|element| self.lower_rvalue_by_id(context, *element))
            .collect();
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Tuple(IrTuple::new(elements)),
            span,
            ty,
        )))
    }
}
