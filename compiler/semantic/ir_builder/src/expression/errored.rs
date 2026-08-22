use rayc_ir::expression::{Expression, ExpressionID};
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::errored::Errored};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Errored>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a Errored>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        self.emit_expression(Expression::new_error(span, ty))
    }
}
