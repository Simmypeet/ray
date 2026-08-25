use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_typed_ast::typed_expr::identifier::Identifier;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Identifier>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Identifier>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let address = self.lower_address_by_id(context, expression.id());
        self.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
    }
}
