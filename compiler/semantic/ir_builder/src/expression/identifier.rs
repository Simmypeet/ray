use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, load::Load};
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::identifier::Identifier};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Identifier>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a Identifier>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let address = self.lower_address_by_id(typed_function, expression.id());
        self.emit_expression(Expression::new(ExpressionKind::Load(Load::new(address)), span, ty))
    }
}
