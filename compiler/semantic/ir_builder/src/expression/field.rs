use rayc_typed_ast::typed_expr::field_access::FieldAccess;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a FieldAccess>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a FieldAccess>,
    ) -> LoweredExpression {
        let operand_id = expression.node().operand();
        let operand = self.lower_by_id(context, operand_id).await;
        let mut address = self.lower_to_address_or_temporary(context, operand_id, operand);
        self.project_field(&mut address, expression.node().field());
        LoweredExpression::LValue(address)
    }
}
