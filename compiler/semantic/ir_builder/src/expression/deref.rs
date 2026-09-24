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
        // The dereference extends the pointer's own place, so the pointer is
        // read where the place is used rather than loaded here. A computed
        // pointer is first stored to a temporary to give it a place.
        let pointer_id = expression.node().pointee();
        let pointer = self.lower_by_id(context, pointer_id).await;
        let mut address = self.lower_to_address_or_temporary(context, pointer_id, pointer);
        self.project_raw_deref(&mut address);
        LoweredExpression::LValue(address)
    }
}
