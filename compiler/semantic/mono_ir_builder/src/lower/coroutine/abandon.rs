//! Lowers the `coroutine_drop` of a coroutine, which drops a frame that has
//! started. A completed frame holds nothing. A suspended one abandons what the
//! function waits for where it is suspended, then runs the abandon block of
//! that suspension point. A frame that has not started is dropped at the drop
//! site instead, with the dictionaries of the `coro` that created it, so
//! `coroutine_drop` never sees one.
//!
//! ```c
//! void coroutine_drop(coroutine_frame_t *frame) {
//!     switch (frame->state) {
//!     case COMPLETED:      return;
//!     case SUSPENDED_AT_p: goto abandon_entry_p;
//!     default:             __builtin_unreachable();   // NOT_STARTED
//!     }
//!
//! abandon_entry_p:                        // abandon what p waits for
//!     callee_drop(&frame->child_p);
//!     goto abandon_block_p;
//!
//! abandon_block_p:                        // the abandon block of p
//!     drop(frame->x); ...                 // what memory analysis put in it
//!     return;
//! }
//! ```

use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{
    cfg::{BlockID as IRBlockID, Terminator as IRTerminator},
    ir_expr::{IRExprID, call::Call as IRCall, run_with::RunWith},
    suspension_point::SuspendingExpr,
};
use rayc_mono_ir::{
    cfg::{BlockID, Terminator},
    layout::{coroutine::FrameState, run_with::RunWithPhase},
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
            let entry = self.lower_abandon_path(point, ctx).await;
            targets.push((frame.state_tag(FrameState::Suspended(point)), entry));
        }

        // A frame that has not started takes the default, which never happens.
        self.select_block(dispatch);
        let state = Operand::Copy(frame.state_place(self.engine()));
        self.set_switch_terminator(state, targets);
    }

    /// Lowers what dropping the frame does while it is suspended at `point`,
    /// and returns the block it starts in.
    ///
    /// What the function waits for is abandoned first. It is deeper than the
    /// function, so its scopes end first.
    ///
    /// ```c
    /// abandon_entry_p:
    ///     <abandon what p waits for>
    ///     goto abandon_block_p;
    /// ```
    async fn lower_abandon_path(&mut self, point: SuspensionPointID, ctx: &Context<'_>) -> BlockID {
        let abandon_block = ctx.source_abandon_block(point);
        self.lower_abandon_block(abandon_block, ctx).await;

        // Abandoning what the function waits for may take several blocks, so
        // the jump ends whichever block is selected after it.
        let entry = self.create_block();
        self.select_block(entry);
        self.abandon_child_frame(point, ctx).await;
        self.set_terminator(Terminator::Goto(ctx.block(abandon_block)));

        entry
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

    /// Abandons the child frame of `point`, the coroutine that the function
    /// owns and waits for while it is suspended there, starting in the
    /// selected block, and leaves the block after it selected.
    ///
    /// Without a child frame there is nothing to abandon. A `perform` waits for
    /// a handler outside the function. The continuation of a `handle` or of
    /// a call of one points to a frame that something else owns: the
    /// storage of some `coro`, which is dropped where that storage ends.
    async fn abandon_child_frame(&mut self, point: SuspensionPointID, ctx: &Context<'_>) {
        if !ctx.frame().holds_child_frame(point) {
            return;
        }

        let expression = ctx.suspending_expression(point);
        match SuspendingExpr::of(ctx.source_expression(expression).kind()) {
            SuspendingExpr::Call(call) => self.abandon_callee(point, call, ctx).await,
            SuspendingExpr::RunWith(run_with) => {
                self.abandon_run_with(point, expression, run_with, ctx).await;
            }
            SuspendingExpr::Perform | SuspendingExpr::Handle(_) => {
                unreachable!("only a callee frame or a `run ... with` is held as a child frame")
            }
        }
    }

    /// Abandons the callee of `call`, the call at `point`, which runs in a
    /// frame of its own: the child frame of `point`.
    ///
    /// ```c
    ///     callee_drop(&frame->child_p);
    /// ```
    async fn abandon_callee(&mut self, point: SuspensionPointID, call: &IRCall, ctx: &Context<'_>) {
        let frame_type = self.callee_frame_type(call).await;
        let callee = frame_type.assert_as_coroutine_frame();
        let destination = self.new_unit_place();
        self.call_coroutine_drop(&ctx.child_frame_place(point), callee, destination).await;
    }

    /// Abandons `run_with`, the `run ... with` expression `expression` at
    /// `point`, and leaves the block after it selected.
    ///
    /// The clause that runs, if any, goes first, since it borrows the captures
    /// of the clauses and the body waits for it. The body goes next, then the
    /// captures: the reverse of the order they were created in.
    ///
    /// ```c
    ///     switch (frame->child_p.phase) {
    ///     case PHASE_BODY:     goto abandon_body;
    ///     case PHASE_CLAUSE_a: goto abandon_clause_a;     // one per clause
    ///     }
    ///
    /// abandon_clause_a:
    ///     clause_a_drop(&frame->child_p.clause_a);
    ///     goto abandon_body;
    ///
    /// abandon_body:                                       // left selected
    ///     body_drop(&frame->child_p.body);
    ///     drop(frame->child_p.handler_environment.capture_n); ...
    /// ```
    async fn abandon_run_with(
        &mut self,
        point: SuspensionPointID,
        expression: IRExprID,
        run_with: &RunWith,
        ctx: &Context<'_>,
    ) {
        let layout = self.resolver().run_with_layout(&ctx.run_with_instance(expression)).await;
        let frames = ctx.child_frame_place(point);
        let destination = self.new_unit_place();
        let entry = self.selected_block();

        // A clause that runs is suspended, and is dropped before the body.
        let abandon_body = self.create_block();
        let mut targets = vec![(layout.phase_tag(RunWithPhase::Body), abandon_body)];
        for clause in layout.clauses() {
            let block = self.create_block();
            self.select_block(block);
            let frame = self.project(&frames).run_with_clause(clause.operation_id());
            self.call_coroutine_drop(&frame, clause.coroutine(), destination.clone()).await;
            self.set_terminator(Terminator::Goto(abandon_body));

            let phase = RunWithPhase::Clause(clause.operation_id());
            targets.push((layout.phase_tag(phase), block));
        }

        // Only the phase tells which clause runs, if any.
        self.select_block(entry);
        let phase = Operand::Copy(self.project(&frames).run_with_phase());
        self.set_switch_terminator(phase, targets);

        // The body is suspended in every phase. The clauses are done with
        // their captures once the clause that runs is dropped.
        self.select_block(abandon_body);
        let body = self.project(&frames).run_with_body();
        self.call_coroutine_drop(&body, layout.body(), destination).await;
        if run_with.handler_capture_map().is_some() {
            let environment = self.project(&frames).run_with_handler_environment();
            self.drop_handler_captures(run_with, &environment, ctx).await;
        }
    }

    /// Ends the selected block by returning unit, as a `Drop.drop` does.
    fn return_unit(&mut self) {
        self.set_terminator(Terminator::Return(Some(Operand::Constant(Constant::Unit))));
    }
}
