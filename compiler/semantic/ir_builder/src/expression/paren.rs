use rayc_ir::expression::ExpressionID;
use rayc_typed_ast::typed_expr::paren::Paren;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Paren>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Paren>,
    ) -> ExpressionID {
        self.lower_expression_by_id(context, expression.node().expression())
    }
}
