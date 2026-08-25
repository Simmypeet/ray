use rayc_ir::ir_expr::{ExpressionID, IRExpr, IRExprKind, call::Call as IrCall};
use rayc_typed_ast::typed_expr::call::{Call, CallTarget};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl Builder {
    fn lower_call_arguments(
        &mut self,
        context: &LoweringContext<'_>,
        call: &Call,
    ) -> Vec<ExpressionID> {
        call.arguments()
            .iter()
            .map(|argument| self.lower_expression_by_id(context, *argument))
            .collect()
    }
}

impl<'a> LowerExpression<TypedExprWithID<&'a Call>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Call>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let call = expression.node();
        // Ray evaluates the callee before call arguments, then arguments from left to
        // right.
        let lowered_call = match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let arguments = self.lower_call_arguments(context, call);
                IrCall::new_direct(*function_id, arguments, subst.clone())
            }
            CallTarget::Lambda { callee } => {
                let callee = self.lower_expression_by_id(context, *callee);
                let arguments = self.lower_call_arguments(context, call);
                IrCall::new_lambda(callee, arguments)
            }
        };
        self.emit_expression(IRExpr::new(IRExprKind::Call(lowered_call), span, ty))
    }
}
