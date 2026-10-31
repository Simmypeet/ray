use rayc_mono_ir::{
    cfg::{BlockID, Switch, Terminator},
    operand::{Constant, Operand},
    ty::CoroutineProgress,
};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    /// Lowers the function in coroutine style.
    pub(crate) async fn lower_coroutine(mut self) {
        let dispatch = self.entry_block();
        self.lower_blocks().await;

        // Every resume point is known now that the blocks are lowered.
        self.select_block(dispatch);
        self.lower_resume_dispatch();
    }

    /// Jumps from the entry block to where the frame's state says to continue.
    fn lower_resume_dispatch(&mut self) {
        let form = self.coroutine_form();
        let state = self.project(form.frame_place()).coroutine_frame_state();

        let targets = std::iter::once((
            form.frame().layout().start_state(),
            self.block(self.source_entry_block()),
        ))
        .chain(form.resume_points())
        .collect::<Vec<_>>();

        self.set_switch_terminator(Operand::Copy(state), targets);
    }

    /// Terminates the current block with a switch whose other values never
    /// happen.
    pub(super) fn set_switch_terminator(
        &mut self,
        value: Operand,
        targets: Vec<(Constant, BlockID)>,
    ) {
        let otherwise = self.in_new_block(|emit| emit.set_terminator(Terminator::Unreachable));
        self.set_terminator(Terminator::Switch(Switch::new(value, targets, otherwise)));
    }

    /// Lowers a `return` of a coroutine, which completes it.
    pub(super) fn lower_coroutine_return(&mut self, value: Operand) {
        let form = self.coroutine_form();
        let state = self.project(form.frame_place()).coroutine_frame_state();
        let progress = form.progress_place().clone();
        let completed_state = form.frame().layout().completed_state();

        self.store_constant(state, completed_state);
        self.store_constant(
            self.project(&progress).coroutine_progress_tag(),
            CoroutineProgress::return_tag(),
        );
        self.store(self.project(&progress).coroutine_progress_return(), value);
        self.set_terminator(Terminator::Return(Some(Operand::Copy(progress))));
    }
}
