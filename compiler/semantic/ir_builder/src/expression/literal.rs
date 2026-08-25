use rayc_ir::expression::{
    Expression, ExpressionID, ExpressionKind, literal::Literal as IrLiteral,
};
use rayc_typed_ast::typed_expr::literal::Literal;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Literal>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Literal>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let literal = expression.node();
        let literal = match literal {
            Literal::Numeric(value) => IrLiteral::Numeric(*value),
            Literal::Bool(value) => IrLiteral::Bool(*value),
        };
        self.emit_expression(Expression::new(ExpressionKind::Literal(literal), span, ty))
    }
}
