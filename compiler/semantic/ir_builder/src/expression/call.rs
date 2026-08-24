use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, call::Call as IrCall};
use rayc_typed_ast::{
    typed_function::TypedFunction as TypedFunction,
    typed_expr::call::{Call, CallTarget},
};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl Builder {
    fn lower_call_arguments(
        &mut self,
        call: &Call,
        typed_function: &TypedFunction,
    ) -> Vec<ExpressionID> {
        call.arguments()
            .iter()
            .map(|argument| self.lower_expression_by_id(typed_function, *argument))
            .collect()
    }
}

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
        // Ray evaluates the callee before call arguments, then arguments from left to
        // right.
        let lowered_call = match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let arguments = self.lower_call_arguments(call, typed_function);
                IrCall::new_direct(*function_id, arguments, subst.clone())
            }
            CallTarget::Lambda { callee } => {
                let callee = self.lower_expression_by_id(typed_function, *callee);
                let arguments = self.lower_call_arguments(call, typed_function);
                IrCall::new_lambda(callee, arguments)
            }
        };
        self.emit_expression(Expression::new(ExpressionKind::Call(lowered_call), span, ty))
    }
}
