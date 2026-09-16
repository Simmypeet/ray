use rayc_ir::ir_expr::{IRExpr, IRExprID, IRExprKind, call::Call as IrCall, perform::Perform};
use rayc_typed_ast::typed_expr::call::{Call, CallTarget};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl Builder {
    fn lower_call_arguments(
        &mut self,
        context: &LoweringContext<'_>,
        call: &Call,
    ) -> Vec<IRExprID> {
        call.arguments()
            .iter()
            .map(|argument| self.lower_rvalue_by_id(context, *argument))
            .collect()
    }
}

impl<'a> Lower<TypedExprWithID<&'a Call>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Call>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let effect = typed_expression.effect().clone();
        let call = expression.node();
        // Ray evaluates the callee before call arguments, then arguments from left to
        // right.
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let arguments = self.lower_call_arguments(context, call);
                let call = IrCall::new_direct(*function_id, arguments, subst.clone(), effect);
                LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                    IRExprKind::Call(call),
                    span,
                    ty,
                )))
            }

            CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            } => {
                let arguments = self.lower_call_arguments(context, call);
                let call = IrCall::new_unresolved_instance_associated(
                    instance.clone(),
                    *trait_def_id,
                    trait_def_subst.clone(),
                    arguments,
                    effect,
                );
                LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                    IRExprKind::Call(call),
                    span,
                    ty,
                )))
            }

            CallTarget::EffectOperation { effect_id, operation_id, subst } => {
                let arguments = self.lower_call_arguments(context, call);
                let perform = Perform::new(*effect_id, *operation_id, arguments, subst.clone());
                LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                    IRExprKind::Perform(perform),
                    span,
                    ty,
                )))
            }
        }
    }
}
