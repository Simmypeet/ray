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
        let closure = expression.node();
        let closure_view = typed_expression.ty().unwrap_as_closure_view();

        assert_eq!(
            context.closure_function(closure_view.local_closure_id()),
            Some(closure.function_id())
        );

        // Keep the source identity even when lowering assigns a different function ID.
        let function_id = self
            .lower_closure_function(
                context,
                closure.function_id(),
                closure_view.return_type().clone(),
                typed_expression.span(),
            )
            .await;
        self.register_closure(closure_view.local_closure_id(), function_id);
        let captures = self.lower_capture_operands(context, closure.function_id());

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Closure(IrClosure::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
