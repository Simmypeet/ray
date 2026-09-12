use rayc_ir::ir_expr::{IRExpr, IRExprID, IRExprKind, nlambda::NLambda as IrNLambda};
use rayc_typed_ast::typed_expr::nlambda::NLambda;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a NLambda>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a NLambda>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        let lambda = expression.node();
        let closure = typed_expression.ty().unwrap_as_closure_view();

        assert_eq!(
            context.closure_function(closure.local_closure_id()),
            Some(lambda.function_id())
        );

        // Keep the source identity even when lowering assigns a different function ID.
        let function_id = self.lower_lambda_function(
            context,
            lambda.function_id(),
            closure.return_type().clone(),
            typed_expression.span(),
        );
        self.register_closure(closure.local_closure_id(), function_id);
        let captures = self.lower_capture_operands(context, lambda.function_id());

        self.emit_expression(IRExpr::new(
            IRExprKind::NLambda(IrNLambda::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}
