use rayc_ir::ir_expr::{
    IRExpr, IRExprKind,
    handle::{Handle, HandledFunction, OperationHandler},
};
use rayc_typed_ast::typed_expr::run_with::RunWith;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a RunWith>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RunWith>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let run_with = expression.node();
        let body_function =
            self.lower_thunk_function(context, run_with.body(), typed_expression.span());
        let body = HandledFunction::new(
            body_function,
            self.lower_capture_operands(context, run_with.body()),
        );

        // Materialize one capture layout and one operand list for the complete
        // operation-handler group.
        let (handler_captures, handler_capture_map, handler_bindings) =
            match run_with.operation_handler_entries().next() {
                Some((_, function_id)) => {
                    assert!(
                        run_with
                            .operation_handlers()
                            .all(|handler| context.shares_capture_plan(handler, function_id)),
                        "operation handlers in one run-with expression should share a capture plan"
                    );
                    let operands = self.lower_capture_operands(context, function_id);
                    let (capture_map, bindings) = self.lower_capture_map(context, function_id);

                    (operands, Some(capture_map), Some(bindings))
                }

                None => (Vec::new(), None, None),
            };

        let handlers = run_with
            .operation_handler_entries()
            .map(|(operation, typed_function_id)| {
                let capture_map_id = handler_capture_map
                    .expect("an operation handler should have a shared capture map");
                let bindings = handler_bindings
                    .as_ref()
                    .expect("an operation handler should have shared bindings");

                let function_id = self.lower_operation_handler_function(
                    context,
                    typed_function_id,
                    typed_expression.span(),
                    capture_map_id,
                    bindings.clone(),
                );
                OperationHandler::new(
                    run_with.effect().target_id.make_global(operation),
                    function_id,
                )
            })
            .collect();

        let handle = Handle::new(
            run_with.effect(),
            run_with.effect_substitution().clone(),
            body,
            handler_captures,
            handler_capture_map,
            handlers,
            typed_expression.effect().clone(),
        );

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Handle(handle),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
