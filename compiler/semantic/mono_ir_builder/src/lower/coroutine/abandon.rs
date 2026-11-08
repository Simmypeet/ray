//! Lowers the `coroutine_drop` of a coroutine, which drops a frame that has
//! started. A completed frame holds nothing, and a suspended one runs the
//! abandon block of the suspension point it is at. A frame that has not
//! started is dropped at the drop site instead, with the dictionaries of the
//! `coro` that created it, so `coroutine_drop` never sees one.
//!
//! ```c
//! void coroutine_drop(coroutine_frame_t *frame) {
//!     switch (frame->state) {
//!     case COMPLETED:      return;
//!     case SUSPENDED_AT_p: goto abandon_block_p;
//!     default:             __builtin_unreachable();   // NOT_STARTED
//!     }
//!
//! abandon_block_p:                        // the abandon block of p
//!     drop(frame->x); ...                 // what memory analysis put in it
//!     return;
//! }
//! ```
//!
//! What the function waits for at `p` is not abandoned yet, so what it holds
//! leaks: the frame of a callee, and the body, the running clause and the
//! handler captures of a `run ... with`.

use rayc_ir::cfg::{BlockID as IRBlockID, Terminator as IRTerminator};
use rayc_mono_ir::{
    cfg::Terminator,
    layout::coroutine::FrameState,
    operand::{Constant, Operand},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers the function into its `coroutine_drop`.
    pub(crate) async fn lower_coroutine_drop(mut self, ctx: &Context<'_>) {
        let dispatch = self.entry_block();
        let frame = ctx.frame();

        // A completed frame holds nothing.
        let completed = self.in_new_block(Self::return_unit);
        let mut targets = vec![(frame.state_tag(FrameState::Completed), completed)];

        // A suspended frame drops what the function holds where it is
        // suspended.
        for point in frame.suspension_points() {
            let abandon_block = ctx.source_abandon_block(point);
            self.lower_abandon_block(abandon_block, ctx).await;
            targets.push((frame.state_tag(FrameState::Suspended(point)), ctx.block(abandon_block)));
        }

        // A frame that has not started takes the default, which never happens.
        self.select_block(dispatch);
        let state = Operand::Copy(frame.state_place(self.engine()));
        self.set_switch_terminator(state, targets);
    }

    /// Lowers `block`, the abandon block of a suspension point, as memory
    /// analysis left it: it drops what the function still holds there, pops
    /// its scopes, and leaves the function.
    ///
    /// ```c
    /// abandon_block_p:
    ///     drop(frame->x); ...
    ///     return;
    /// ```
    async fn lower_abandon_block(&mut self, block: IRBlockID, ctx: &Context<'_>) {
        self.select_block(ctx.block(block));
        for instruction in ctx.source_block_instructions(block) {
            self.lower_instruction(instruction, ctx).await;
        }

        // The IR builder ends every abandon block with `Abandon`, and memory
        // analysis never splits an edge into one, since its suspension point
        // is its only predecessor. So the path is this one block.
        assert!(
            ctx.source_block_terminator(block).is_some_and(IRTerminator::is_abandon),
            "an abandon block should leave the function by itself"
        );
        self.return_unit();
    }

    /// Ends the selected block by returning unit, as a `Drop.drop` does.
    fn return_unit(&mut self) {
        self.set_terminator(Terminator::Return(Some(Operand::Constant(Constant::Unit))));
    }
}
