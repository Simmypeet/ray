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
        let fields = closure.captures().iter().map(|capture| ctx.expression_operand(*capture));

        // Effect handlers are passed as additional arguments to the function,
        // not embedded in the environment.
        assert_eq!(environment_abi.captured_effects().len(), 0);

        let destination = ctx.expression_place(expression_id);
        self.initialize_environment(&destination, fields);
    }
}
