use rayc_ir::ir_expr::{IRExpr, IRExprKind, literal::Literal as IrLiteral};
use rayc_typed_ast::typed_expr::literal::Literal;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Literal>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Literal>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let literal = expression.node();
        let literal = match literal {
            Literal::Numeric(value) => IrLiteral::Numeric(*value),
            Literal::Bool(value) => IrLiteral::Bool(*value),
            Literal::String(value) => IrLiteral::String(value.clone()),
        };
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Literal(literal),
            span,
            ty,
        )))
    }
}
