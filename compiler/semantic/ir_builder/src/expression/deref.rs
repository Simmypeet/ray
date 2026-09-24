use rayc_typed_ast::typed_expr::deref::Deref;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Deref>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Deref>,
    ) -> LoweredExpression {
        let pointer = self.lower_rvalue_by_id(context, expression.node().pointee()).await;
        LoweredExpression::LValue(self.dereference_address(pointer))
    }
}
