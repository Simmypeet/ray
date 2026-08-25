use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, tuple::Tuple as IrTuple};
use rayc_typed_ast::typed_expr::tuple::Tuple;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Tuple>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Tuple>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let tuple = expression.node();
        let elements = tuple
            .elements()
            .iter()
            .map(|element| self.lower_expression_by_id(context, *element))
            .collect();
        self.emit_expression(Expression::new(
            ExpressionKind::Tuple(IrTuple::new(elements)),
            span,
            ty,
        ))
    }
}
