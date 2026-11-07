use rayc_ir::ir_expr::{IRExprID, closure::Closure};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    pub(super) fn lower_closure(
        &mut self,
        closure: &Closure,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        // Captures have already been evaluated in semantic IR order.
        let environment_abi = ctx.function_environment_abi(closure.function_id());
        assert_eq!(closure.captures().len(), environment_abi.capture_ids().len());
        let captures = environment_abi.capture_ids().map(|capture| {
            (capture, ctx.expression_operand(closure.captures().get_operand_of(capture)))
        });

        // The value of a closure is its environment.
        assert_eq!(**ctx.expression_type(expression_id), environment_abi.environment_type());

        // Effect handlers are passed as additional arguments to the function,
        // not embedded in the environment.
        assert_eq!(environment_abi.layout().handler_records().len(), 0);

        let destination = ctx.expression_place(expression_id);
        self.initialize_environment(&destination, captures, []);
    }
}
