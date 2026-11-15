use rayc_ir::ir_expr::{IRExpr, IRExprKind, variant::Variant};
use rayc_typed_ast::typed_expr::variant::Variant as TypedVariant;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a TypedVariant>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a TypedVariant>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let variant = expression.node();

        // The payload is evaluated into a value, which the variant then owns.
        let payload = match variant.payload() {
            Some(payload) => Some(self.lower_rvalue_by_id(context, payload).await),
            None => None,
        };

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Variant(Variant::new(variant.variant_id(), payload)),
            span,
            ty,
        )))
    }
}
