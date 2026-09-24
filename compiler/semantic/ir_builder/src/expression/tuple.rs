use rayc_ir::ir_expr::{IRExpr, IRExprKind, tuple::Tuple as IrTuple};
use rayc_typed_ast::typed_expr::tuple::Tuple;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Tuple>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Tuple>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let tuple = expression.node();
        let mut elements = Vec::with_capacity(tuple.elements().len());
        for element in tuple.elements() {
            elements.push(self.lower_rvalue_by_id(context, *element).await);
        }
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Tuple(IrTuple::new(elements)),
            span,
            ty,
        )))
    }
}
