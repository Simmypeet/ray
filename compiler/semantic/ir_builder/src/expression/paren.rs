use rayc_typed_ast::typed_expr::paren::Paren;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Paren>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Paren>,
    ) -> LoweredExpression {
        self.lower_by_id(context, expression.node().expression()).await
    }
}
