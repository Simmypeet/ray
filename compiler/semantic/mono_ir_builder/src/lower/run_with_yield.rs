//! Lowers a `run ... with` at a suspension point of a `coroutine_resume`: one
//! whose residual effect is not empty, so its body and its clauses yield out
//! of the function.
//!
//! The body and each clause run as coroutines of their own, which this
//! function resumes in poll loops. All of them live in the awaitee of the
//! suspension point, with the environment that the clauses share, and the
//! phase of the awaitee tells which one runs. At most one clause runs at a
//! time, since the body waits at its operation until the clause returns.
//!
//! ```c
//! // the block that evaluates the `run ... with`
//!     frame->awaitee_p.handler_environment = { .capture0 = ..., ... };
//!     frame->awaitee_p.body = { .state = NOT_STARTED, .capture3 = ..., ... };
//!     frame->awaitee_p.phase = PHASE_BODY;
//!     body_args.unit = ();
//!     frame->state = SUSPENDED_AT_p;          // every phase shares it
//!     goto poll_body;
//!
//! resume_entry_p:                             // the result of a yielded operation
//!     switch (frame->awaitee_p.phase) {
//!     case PHASE_BODY:     body_args = (body_resume_t) args; goto poll_body;
//!     case PHASE_CLAUSE_a: a_args = (a_resume_t) args;       goto poll_a;
//!     }
//!
//! poll_body:                                  // poll.rs
//!     progress = body_resume(&frame->awaitee_p.body, body_args);
//!     switch (progress.tag) {
//!     case RETURN: goto body_return_arm;
//!     case YIELD:
//!         switch (progress.yield.tag) {
//!         case E_a: goto start_clause_a;
//!         case R_x: ...forward...;            // forward.rs
//!         }
//!     }
//!
//! body_return_arm:
//!     drop(frame->awaitee_p.handler_environment.capture_n); ...;
//!     result = progress.return_value;
//!     goto after_p;
//!
//! start_clause_a:
//!     frame->awaitee_p.clause_a = {
//!         .state = NOT_STARTED,
//!         .environment = &frame->awaitee_p.handler_environment,
//!         .param0 = progress.yield.E_a.elem0,
//!     };
//!     frame->awaitee_p.phase = PHASE_CLAUSE_a;
//!     a_args.unit = ();
//!     goto poll_a;
//!
//! poll_a:
//!     a_progress = a_resume(&frame->awaitee_p.clause_a, a_args);
//!     switch (a_progress.tag) {
//!     case RETURN: goto clause_return_arm_a;
//!     case YIELD:  ...forward...;             // a clause performs only R
//!     }
//!
//! clause_return_arm_a:                        // the result of E.a resumes the body
//!     body_args.<member of a's result type> = a_progress.return_value;
//!     frame->awaitee_p.phase = PHASE_BODY;
//!     goto poll_body;
//! ```
//!
//! Every jump between the loops stays in one call of `coroutine_resume`, so
//! nothing but the awaitee lives across a return from it.

use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{cfg::Suspend, ir_expr::run_with::RunWith};
use rayc_mono_ir::{
    cfg::{BlockID, Terminator},
    layout::{
        coroutine::{RowOperationID, RowOperationKey},
        run_with::{RunWithLayout, RunWithPhase},
    },
    operand::Operand,
    place::Place,
};
use rayc_symbol::GlobalSymbolID;

use super::poll::PollLoop;
use crate::{context::Context, emitter::Emitter};

/// The poll loop of the clause of one operation.
struct ClauseLoop {
    operation_id: GlobalSymbolID,
    /// The operation in the row of the body, which yields it.
    operation: RowOperationID,
    poll: PollLoop,
}

/// What the lowering of a `run ... with` at one suspension point refers to.
struct RunWithSite<'a> {
    point: SuspensionPointID,
    run_with: &'a RunWith,
    layout: &'a RunWithLayout,
    /// The awaitee of the suspension point.
    awaitee: Place,
}

impl Emitter<'_> {
    /// Lowers a `run ... with` that suspends at `point`, ending the current
    /// block, and continues in the lowering of the resume block of `suspend`.
    ///
    /// It emits these blocks, each built by the helper named beside it:
    ///
    /// ```c
    ///     <start the body>                    // start_run_with
    /// resume_entry_p:                         // fill_run_with_resume_entry
    ///     switch (frame->awaitee_p.phase) { ... }
    ///
    /// poll_body:                              // create_held_frame_poll, finish_poll
    ///     progress = body_resume(&frame->awaitee_p.body, body_args);
    ///     switch (progress.tag) {
    ///     case RETURN: goto body_return_arm;  // body_return_arm
    ///     case YIELD:
    ///         switch (progress.yield.tag) {
    ///         case E_a: goto start_clause_a;  // start_clause_arm
    ///         case R_x: ...forward...;        // forward.rs
    ///         }
    ///     }
    ///
    /// poll_a:                                 // one per clause, from create_clause_loops
    ///     a_progress = a_resume(&frame->awaitee_p.clause_a, a_args);
    ///     switch (a_progress.tag) {
    ///     case RETURN: goto clause_return_arm_a;  // clause_return_arm
    ///     case YIELD:  ...forward...;
    ///     }
    /// ```
    pub(super) async fn lower_run_with_suspension(
        &mut self,
        point: SuspensionPointID,
        suspend: &Suspend,
        run_with: &RunWith,
        ctx: &Context<'_>,
    ) {
        let instance = ctx.run_with_instance(suspend.expression());
        let layout = self.resolver().run_with_layout(&instance).await;
        let site =
            RunWithSite { point, run_with, layout: &layout, awaitee: ctx.awaitee_place(point) };

        // The loops are created first, since their blocks jump to each other.
        let body = self
            .create_held_frame_poll(
                point,
                layout.body().clone(),
                self.project(&site.awaitee).run_with_body(),
            )
            .await;
        let clauses = self.create_clause_loops(&site, &body, ctx).await;

        // Start the body, and let the resume entry continue whichever loop
        // yielded.
        self.start_run_with(&site, &body, ctx).await;
        self.fill_run_with_resume_entry(&site, &body, &clauses, ctx);

        // The body completes the `run ... with`, or starts the clause of an
        // operation it handles, and forwards any other operation.
        let done = self.lower_edge(point.block_id(), suspend.resume_block(), ctx);
        let destination = ctx.expression_place(suspend.expression());
        let return_arm = self.body_return_arm(&site, &body, destination, done, ctx).await;
        let mut handled = Vec::with_capacity(clauses.len());
        for clause in &clauses {
            let arm = self.start_clause_arm(&site, &body, clause).await;
            handled.push((clause.operation, arm));
        }
        self.finish_poll(&body, return_arm, &handled, ctx);

        // A clause resumes the body with its result, and forwards every
        // operation, since it performs only the residual effect.
        for clause in &clauses {
            let return_arm = self.clause_return_arm(&site, &body, clause);
            self.finish_poll(&clause.poll, return_arm, &[], ctx);
        }
    }

    /// Creates the poll loop of each clause, whose frame is in the union of
    /// the awaitee. Nothing enters a loop yet, and its poll block has no
    /// terminator until [`Self::finish_poll`] adds the switch:
    ///
    /// ```c
    /// poll_a:                                 // one per clause
    ///     frame_pointer = &frame->awaitee_p.clause_a;
    ///     a_progress = a_resume(frame_pointer, a_args);
    /// ```
    ///
    /// It also looks up the operation of each clause in the row of `body`,
    /// which the body yields it as.
    async fn create_clause_loops(
        &mut self,
        site: &RunWithSite<'_>,
        body: &PollLoop,
        ctx: &Context<'_>,
    ) -> Vec<ClauseLoop> {
        // The body yields the operations of the handled effect, which its row
        // names as the `run ... with` instantiates it.
        let run_with = site.run_with;
        let effect = ctx.instantiate_effect(run_with.effect_id(), run_with.substitution()).await;

        let mut clauses = Vec::with_capacity(site.layout.clauses().len());
        for clause in site.layout.clauses() {
            let operation_id = clause.operation_id();
            let frame = self.project(&site.awaitee).run_with_clause(operation_id);
            let poll =
                self.create_held_frame_poll(site.point, clause.coroutine().clone(), frame).await;
            let operation = body.row_operation(&RowOperationKey::new(operation_id, effect.clone()));
            clauses.push(ClauseLoop { operation_id, operation, poll });
        }

        clauses
    }

    /// Ends the current block by creating the environment of the clauses and
    /// the frame of the body in the awaitee, and entering the body's loop.
    ///
    /// ```c
    /// // the block that evaluates the `run ... with`
    ///     frame->awaitee_p.handler_environment = { .capture0 = ..., ... };  // with captures only
    ///     frame->awaitee_p.body = { .state = NOT_STARTED, .capture3 = ..., ... };
    ///     frame->awaitee_p.phase = PHASE_BODY;
    ///     body_args.unit = ();
    ///     frame->state = SUSPENDED_AT_p;          // every phase shares it
    ///     goto poll_body;
    /// ```
    async fn start_run_with(&mut self, site: &RunWithSite<'_>, body: &PollLoop, ctx: &Context<'_>) {
        let run_with = site.run_with;

        // The clauses only borrow their captures, so they live in the
        // awaitee. The handler records of the environment stay unset: a
        // clause yields its operations rather than call a handler.
        if let Some(capture_map) = run_with.handler_capture_map() {
            let layout = ctx.capture_environment_layout(capture_map).await;
            let handler_captures = run_with.handler_captures();
            let captures = layout.capture_ids().map(|capture| {
                (capture, ctx.expression_operand(handler_captures.get_operand_of(capture)))
            });
            let environment = self.project(&site.awaitee).run_with_handler_environment();
            self.initialize_environment(&environment, captures, []);
        }

        // The body owns its captures, which move into its frame.
        let frame = self.project(&site.awaitee).run_with_body();
        self.initialize_run_with_body(&frame, site.layout.body(), run_with.captures(), ctx).await;

        // Every phase suspends at the same point, so the state is stored once.
        self.store_phase(site, RunWithPhase::Body);
        self.store_start_resume_args(body);
        self.store_suspended_state(site.point, ctx);
        self.set_terminator(Terminator::Goto(body.block()));
    }

    /// Fills the resume entry of the suspension point, which continues the
    /// loop of the phase with the result of the operation it yielded.
    ///
    /// ```c
    /// resume_entry_p:
    ///     switch (frame->awaitee_p.phase) {
    ///     case PHASE_BODY:     goto resume_body;
    ///     case PHASE_CLAUSE_a: goto resume_a;     // one per clause
    ///     }
    ///
    /// resume_body:
    ///     body_args = (body_resume_t) args;       // `args` as is, if the rows match
    ///     goto poll_body;
    ///
    /// resume_a:
    ///     a_args = (a_resume_t) args;
    ///     goto poll_a;
    /// ```
    fn fill_run_with_resume_entry(
        &mut self,
        site: &RunWithSite<'_>,
        body: &PollLoop,
        clauses: &[ClauseLoop],
        ctx: &Context<'_>,
    ) {
        let loops = std::iter::once((RunWithPhase::Body, body)).chain(
            clauses.iter().map(|clause| (RunWithPhase::Clause(clause.operation_id), &clause.poll)),
        );

        // One block per phase converts the result to the row of its loop.
        let mut targets = Vec::with_capacity(clauses.len() + 1);
        for (phase, poll) in loops {
            let arguments = self.forwarded_resume_args(poll.row(), ctx);
            let args = poll.args().clone();
            let block = poll.block();
            let entry = self.in_new_block(|emit| {
                emit.assign(args, arguments);
                emit.set_terminator(Terminator::Goto(block));
            });
            targets.push((site.layout.phase_tag(phase), entry));
        }

        let phase = Operand::Copy(self.project(&site.awaitee).run_with_phase());
        self.fill_resume_entry(site.point, ctx, |emit| emit.set_switch_terminator(phase, targets));
    }

    /// Creates the arm taken when the body completed, which completes the
    /// `run ... with` with its result in `destination`, and continues in
    /// `done`.
    ///
    /// The clauses are done with their captures by then, so they are dropped
    /// first. The result is stored only afterwards, since `destination` may
    /// share memory with the awaitee.
    ///
    /// ```c
    /// body_return_arm:
    ///     drop(frame->awaitee_p.handler_environment.capture_n); ...;  // with captures only
    ///     destination = progress.return_value;
    ///     goto done;
    /// ```
    async fn body_return_arm(
        &mut self,
        site: &RunWithSite<'_>,
        body: &PollLoop,
        destination: Place,
        done: BlockID,
        ctx: &Context<'_>,
    ) -> BlockID {
        let arm = self.create_block();
        let previous = self.selected_block();
        self.select_block(arm);

        if site.run_with.handler_capture_map().is_some() {
            let environment = self.project(&site.awaitee).run_with_handler_environment();
            self.drop_handler_captures(site.run_with, &environment, ctx).await;
        }
        let value = self.project(body.progress()).coroutine_progress_return_payload();
        self.store(destination, Operand::Copy(value));
        self.set_terminator(Terminator::Goto(done));

        self.select_block(previous);
        arm
    }

    /// Creates the arm that starts the clause of an operation that the body
    /// yielded, with the arguments of the operation.
    ///
    /// The clause's frame shares the union of the awaitee with the frames of
    /// the other clauses, which are not running, since the body waits at its
    /// operation until the clause returns.
    ///
    /// ```c
    /// start_clause_a:
    ///     frame->awaitee_p.clause_a = {
    ///         .state = NOT_STARTED,
    ///         .environment = &frame->awaitee_p.handler_environment,
    ///         .param0 = progress.yield.E_a.elem0, ...
    ///     };
    ///     frame->awaitee_p.phase = PHASE_CLAUSE_a;
    ///     a_args.unit = ();
    ///     goto poll_a;
    /// ```
    async fn start_clause_arm(
        &mut self,
        site: &RunWithSite<'_>,
        body: &PollLoop,
        clause: &ClauseLoop,
    ) -> BlockID {
        let arm = self.create_block();
        let previous = self.selected_block();
        self.select_block(arm);

        let frame = self.project(&site.awaitee).run_with_clause(clause.operation_id);
        let environment = self.project(&site.awaitee).run_with_handler_environment();
        let arguments = self.yielded_arguments(body, clause.operation);
        let instance = site.layout.clause(clause.operation_id).coroutine();
        self.initialize_run_with_clause(&frame, instance, environment, &arguments).await;

        self.store_phase(site, RunWithPhase::Clause(clause.operation_id));
        self.store_start_resume_args(&clause.poll);
        self.set_terminator(Terminator::Goto(clause.poll.block()));

        self.select_block(previous);
        arm
    }

    /// Creates the arm taken when a clause completed, which resumes the body
    /// with the result of its operation.
    ///
    /// ```c
    /// clause_return_arm_a:
    ///     body_args.<member of a's result type> = a_progress.return_value;
    ///     frame->awaitee_p.phase = PHASE_BODY;
    ///     goto poll_body;
    /// ```
    fn clause_return_arm(
        &mut self,
        site: &RunWithSite<'_>,
        body: &PollLoop,
        clause: &ClauseLoop,
    ) -> BlockID {
        let engine = self.engine();
        let result_type = body.row_layout()[clause.operation].resume_type();
        let destination = body.resume_value_place(result_type, engine);
        let value = self.project(clause.poll.progress()).coroutine_progress_return_payload();
        let phase = self.project(&site.awaitee).run_with_phase();
        let body_phase = site.layout.phase_tag(RunWithPhase::Body);
        let body_block = body.block();

        self.in_new_block(|emit| {
            emit.store(destination, Operand::Copy(value));
            emit.store_constant(phase, body_phase);
            emit.set_terminator(Terminator::Goto(body_block));
        })
    }

    /// Records in the awaitee which of the body and the clauses runs, in the
    /// selected block.
    ///
    /// ```c
    ///     frame->awaitee_p.phase = PHASE_BODY | PHASE_CLAUSE_a;
    /// ```
    fn store_phase(&mut self, site: &RunWithSite<'_>, phase: RunWithPhase) {
        let place = self.project(&site.awaitee).run_with_phase();
        self.store_constant(place, site.layout.phase_tag(phase));
    }
}
