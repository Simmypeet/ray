use rayc_ir::ir_expr::{
    IRExpr, IRExprID, IRExprKind,
    handle::{Handle, HandledFunction, OperationHandler},
};
use rayc_typed_ast::typed_expr::run_with::RunWith;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a RunWith>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RunWith>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        let run_with = expression.node();
        let body_function =
            self.lower_thunk_function(context, run_with.body(), typed_expression.span());
        let body = HandledFunction::new(
            body_function,
            self.lower_capture_operands(context, run_with.body()),
        );

        let handlers = run_with
            .operation_handler_entries()
            .map(|(operation, typed_function_id)| {
                let function_id = self.lower_operation_handler_function(
                    context,
                    typed_function_id,
                    typed_expression.span(),
                );
                let captures = self.lower_capture_operands(context, typed_function_id);
                OperationHandler::new(
                    run_with.effect().target_id.make_global(operation),
                    HandledFunction::new(function_id, captures),
                )
            })
            .collect();
        let handle = Handle::new(
            run_with.effect(),
            run_with.effect_substitution().clone(),
            body,
            handlers,
            typed_expression.effect().clone(),
        );
        self.emit_expression(IRExpr::new(
            IRExprKind::Handle(handle),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}
