use rayc_ir::ir_expr::{IRExpr, IRExprKind, closure::Closure as IrClosure};
use rayc_typed_ast::typed_expr::closure::Closure;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Closure>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Closure>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let lambda = expression.node();
        let closure = typed_expression.ty().unwrap_as_closure_view();

        assert_eq!(
            context.closure_function(closure.local_closure_id()),
            Some(lambda.function_id())
        );

        // Keep the source identity even when lowering assigns a different function ID.
        let function_id = self
            .lower_lambda_function(
                context,
                lambda.function_id(),
                closure.return_type().clone(),
                typed_expression.span(),
            )
            .await;
        self.register_closure(closure.local_closure_id(), function_id);
        let captures = self.lower_capture_operands(context, lambda.function_id());

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Closure(IrClosure::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
