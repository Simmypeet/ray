use rayc_mono_ir::{layout::coroutine::FrameState, operand::Operand};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers the function in coroutine style.
    pub(crate) async fn lower_coroutine(mut self, ctx: &Context<'_>) {
        // The entry block of `coroutine_resume` is the dispatch, which is
        // filled last, once lowering the blocks has filled every resume entry.
        let dispatch = self.entry_block();
        self.lower_blocks(ctx).await;

        self.select_block(dispatch);
        self.lower_resume_dispatch(ctx);
    }

    /// Jumps from the entry block to where the frame's state says to continue:
    /// the source entry if the coroutine has not started, or else the resume
    /// entry of the suspension point it is suspended at.
    fn lower_resume_dispatch(&mut self, ctx: &Context<'_>) {
        let start = ctx.block(ctx.source_entry_block());
        let form = ctx.coroutine_form();
        let state = self.project(form.frame_place()).coroutine_frame_state();

        let targets = std::iter::once((form.frame_state_tag(FrameState::NotStarted), start))
            .chain(form.resume_points())
            .collect::<Vec<_>>();

        self.set_switch_terminator(Operand::Copy(state), targets);
    }
}
