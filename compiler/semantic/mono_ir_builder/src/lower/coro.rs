use rayc_ir::ir_expr::{IRExprID, coroutine::Coroutine};
use rayc_mono_ir::operand::Operand;

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    /// Stores the arguments of a `coro` in a frame that has not started.
    pub(super) async fn lower_coro(&mut self, coroutine: &Coroutine, expression_id: IRExprID) {
        let engine = self.engine();
        let destination = self.expression_place(expression_id);
        let instance = self.expression_type(expression_id).assert_as_coroutine_frame().clone();
        let frame = self.resolver().coroutine_frame(&instance).await;

        self.store_constant(
            self.project(&destination).coroutine_frame_state(),
            frame.layout().start_state(),
        );

        let arguments = coroutine
            .arguments()
            .iter()
            .map(|argument| self.expression_place(*argument))
            .collect::<Vec<_>>();

        for (upvar, slot) in frame.upvars() {
            self.store(
                slot.project(&destination, engine),
                Operand::Copy(upvar.place(&arguments, engine)),
            );
        }
    }
}
