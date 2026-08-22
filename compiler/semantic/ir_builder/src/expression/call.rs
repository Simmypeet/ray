use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, call::Call as IrCall};
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::call::Call};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Call>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a Call>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let call = expression.node();
        let arguments = call
            .arguments()
            .iter()
            .map(|argument| self.lower_expression_by_id(typed_function, *argument))
            .collect();
        self.emit_expression(Expression::new(
            ExpressionKind::Call(IrCall::new(call.function_id(), arguments)),
            span,
            ty,
        ))
    }
}
