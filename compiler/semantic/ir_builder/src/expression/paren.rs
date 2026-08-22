use rayc_ir::expression::ExpressionID;
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::paren::Paren};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Paren>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a Paren>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        self.lower_expression_by_id(typed_function, expression.node().expression())
    }
}
