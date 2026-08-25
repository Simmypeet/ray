use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_typed_ast::typed_expr::deref::Deref;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Deref>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Deref>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let address = self.lower_address_by_id(context, expression.id());
        self.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
    }
}
