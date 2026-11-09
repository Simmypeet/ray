//! Drives a coroutine from the function being lowered until it completes: a
//! suspending call, a call of a continuation, and a `handle` all lower to the
//! same poll loop.
//!
//! ```c
//!     args = <first resume value>;         // unit for a new frame
//!     frame->state = SUSPENDED_AT_p;       // only at a suspension point
//!     goto poll;
//!
//! resume_entry_p:                          // only at a suspension point
//!     args = (callee_resume_t) own_args;   // the result of a forwarded operation
//!     goto poll;
//!
//! poll:
//!     frame_pointer = &frame->child | continuation.frame;
//!     progress = callee_resume(frame_pointer, args);
//!     switch (progress.tag) {
//!     case RETURN: result = progress.return_value; goto done;    // return arm
//!     case YIELD:
//!         switch (progress.yield.tag) {
//!         case HANDLED_OP: ...clause...;                         // handle only
//!         case OTHER_OP:   ...forward...;                        // forward.rs
//!         }
//!     }
//! ```
//!
//! [`PollSite`] tells whether the loop is at a suspension point, and
//! [`Resumee`] how the function holds the frame it resumes.

use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::cfg::BlockID as IRBlockID;
use rayc_mono_ir::{
    MonoCoroutineInstance, MonoFunctionInstance,
    cfg::{BlockID, Terminator},
    instruction::Call,
    layout::coroutine::{EffectRowLayout, RowOperationID, RowOperationKey},
    operand::{Constant, Operand},
    place::Place,
    rvalue::{AddressOf, Reinterpret, Rvalue},
    signature::CoroutineSignature,
    ty::{
        AggregateType, CoroutineProgress, CoroutineResume, EffectRow, MonoType, PointerMutability,
    },
};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::struct_body::FieldID;

use crate::{context::Context, emitter::Emitter, resolver::Resolver};

/// Where a poll loop is in the function being lowered.
#[derive(Debug, Clone, Copy)]
pub(super) enum PollSite {
    /// Not at a suspension point: the loop runs to completion without leaving
    /// the function, calling a handler for each operation it forwards.
    Inline,

    /// At a suspension point of a `coroutine_resume`, which leaves it to
    /// forward an operation and is re-entered at its resume entry.
    Suspending(SuspensionPointID),
}

impl PollSite {
    /// Whether the loop leaves `coroutine_resume` to forward an operation.
    pub(super) const fn is_suspending(self) -> bool {
        match self {
            Self::Inline => false,
            Self::Suspending(_) => true,
        }
    }

    /// Where a poll loop that `block` of the function of `ctx` ends with is.
    pub(super) fn of(block: IRBlockID, ctx: &Context<'_>) -> Self {
        ctx.suspension_point(block).map_or(Self::Inline, Self::Suspending)
    }
}

/// How the function holds the frame of the coroutine it resumes, and what it
/// first resumes it with.
pub(super) enum Resumee {
    /// A frame of a callee that has not started, which the function has
    /// created in its own frame. It is first resumed with unit.
    NewFrame { frame: Place, frame_type: Interned<MonoType> },

    /// A `core.Continuation`, which points to a frame somewhere else. It is
    /// first resumed with `value`.
    Continuation {
        continuation: Place,
        continuation_type: Interned<MonoType>,
        value: Operand,
        value_type: Interned<MonoType>,
    },
}

impl Resumee {
    /// What the coroutine is first resumed with, and its type: unit for a
    /// frame that has not started, or the value a continuation is called
    /// with.
    fn first_resume_value(&self, engine: &TrackedEngine) -> (Operand, Interned<MonoType>) {
        match self {
            Self::NewFrame { .. } => {
                (Operand::Constant(Constant::Unit), MonoType::new_unit(engine))
            }
            Self::Continuation { value, value_type, .. } => (value.clone(), value_type.clone()),
        }
    }

    /// The place the poll block reads the frame from: the frame itself, or
    /// the continuation that points to it.
    ///
    /// At a suspension point, the frame layout keeps either in the frame of
    /// the function, since nothing else outlives a return from
    /// `coroutine_resume`: a new frame as the child frame of the point, and
    /// the operand that holds a continuation as a value held across it.
    const fn place(&self) -> &Place {
        match self {
            Self::NewFrame { frame, .. } => frame,
            Self::Continuation { continuation, .. } => continuation,
        }
    }
}

/// A coroutine that the function being lowered resumes.
struct ResumedCoroutine {
    instance: MonoCoroutineInstance,
    signature: Interned<CoroutineSignature>,
    row_layout: Interned<EffectRowLayout>,
}

/// A poll loop whose poll block resumes the coroutine, and whose arms are yet
/// to be built.
pub(super) struct PollLoop {
    resumed: ResumedCoroutine,
    site: PollSite,
    /// The block that resumes the coroutine, which a forwarded operation
    /// loops back to.
    block: BlockID,
    /// The local that holds what the coroutine is resumed with.
    args: Place,
    /// The local that holds what the resume returned.
    progress: Place,
    /// The local that holds the pointer to the frame that was resumed.
    frame_pointer: Place,
}

impl PollLoop {
    pub(super) const fn site(&self) -> PollSite { self.site }

    pub(super) const fn block(&self) -> BlockID { self.block }

    pub(super) const fn args(&self) -> &Place { &self.args }

    pub(super) const fn progress(&self) -> &Place { &self.progress }

    pub(super) const fn frame_pointer(&self) -> &Place { &self.frame_pointer }

    pub(super) const fn row_layout(&self) -> &Interned<EffectRowLayout> { &self.resumed.row_layout }

    /// The row of the resumed coroutine.
    pub(super) fn row(&self) -> &EffectRow { self.resumed.signature.row() }

    /// The member of `args` that holds a value of `ty`, which the resumed
    /// coroutine is resumed with.
    pub(super) fn resume_value_place(
        &self,
        ty: &Interned<MonoType>,
        engine: &TrackedEngine,
    ) -> Place {
        let member = self.resumed.row_layout.get_resume_variant_id(ty);
        self.args.project(engine).coroutine_resume_value(member)
    }

    /// Whether the resumed coroutine returns a progress of the type `own`.
    pub(super) fn has_progress_type_of(&self, own: &CoroutineSignature) -> bool {
        own.progress() == self.resumed.signature.progress()
    }

    /// The operation of the resumed coroutine's row with `key`.
    pub(super) fn row_operation(&self, key: &RowOperationKey) -> RowOperationID {
        self.resumed.row_layout.get_row_operation_id(key)
    }
}

/// The field of a `core.Continuation`, which holds the frame pointer, with the
/// type of the pointer.
pub(super) async fn continuation_frame_field(
    resolver: &Resolver,
    continuation: &MonoType,
) -> (FieldID, Interned<MonoType>) {
    let layout = resolver.struct_layout(continuation.assert_as_struct()).await;
    let mut fields = layout.fields().iter();
    let (field, ty) = fields.next().expect("`core.Continuation` should have a field");
    assert!(fields.next().is_none(), "`core.Continuation` should only hold the frame pointer");

    (*field, ty.clone())
}

/// The `coroutine_resume_t` of `row`.
fn resume_type(row: &EffectRow, engine: &TrackedEngine) -> Interned<MonoType> {
    MonoType::new_aggregate(
        AggregateType::CoroutineResume(CoroutineResume::new(row.clone())),
        engine,
    )
}

impl Emitter<'_> {
    /// Ends the current block by entering a new poll loop that resumes
    /// `instance`, and leaves the poll block selected with the resume call in
    /// it. [`Self::finish_poll`] terminates it.
    ///
    /// ```c
    ///     args.<member of v's type> = v;          // v is unit for a new frame
    ///     <enter_poll_block>                      // ends with `goto poll;`
    /// poll:                                       // left selected
    ///     frame_pointer = <load_frame_pointer>;
    ///     progress = callee_resume(frame_pointer, args);
    ///     // no terminator yet: `finish_poll` adds the switch
    /// ```
    pub(super) async fn begin_poll(
        &mut self,
        site: PollSite,
        instance: MonoCoroutineInstance,
        resumee: Resumee,
        ctx: &Context<'_>,
    ) -> PollLoop {
        let coroutine = self.resumed_coroutine(instance).await;
        let args = self.new_temporary_place(resume_type(coroutine.signature.row(), self.engine()));

        // `args` holds the first resume value in the member of its type.
        let (value, value_type) = resumee.first_resume_value(self.engine());
        let member = coroutine.row_layout.get_resume_variant_id(&value_type);
        self.store(self.project(&args).coroutine_resume_value(member), value);

        let block = self.enter_poll_block(site, coroutine.signature.row(), &args, ctx);

        // The frame pointer is taken in the poll block, since nothing but the
        // frame outlives a return from `coroutine_resume`. It is also copied
        // out first: the values a clause receives may share memory with the
        // operand that holds the continuation.
        let frame_pointer = self.load_frame_pointer(&resumee).await;
        let progress = self.push_resume_call(&coroutine, &frame_pointer, &args);

        PollLoop { resumed: coroutine, site, block, args, progress, frame_pointer }
    }

    /// Creates a poll loop that resumes the frame of `instance` at `frame`,
    /// which the child frame of `point` holds, without entering it. Its poll
    /// block is filled, and the current block stays selected.
    ///
    /// Several such loops share the suspension point, so the caller enters
    /// them, and fills the resume entry of the point: it sets
    /// [`PollLoop::args`] and jumps to [`PollLoop::block`].
    ///
    /// ```c
    /// poll:
    ///     frame_pointer = &frame;
    ///     progress = callee_resume(frame_pointer, args);
    ///     // no terminator yet: `finish_poll` adds the switch
    /// ```
    pub(super) async fn create_held_frame_poll(
        &mut self,
        point: SuspensionPointID,
        instance: MonoCoroutineInstance,
        frame: Place,
    ) -> PollLoop {
        let engine = self.engine();
        let coroutine = self.resumed_coroutine(instance).await;
        let args = self.new_temporary_place(resume_type(coroutine.signature.row(), engine));
        let frame_type = MonoType::new_aggregate(
            AggregateType::CoroutineFrame(coroutine.instance.clone()),
            engine,
        );

        let block = self.create_block();
        let (frame_pointer, progress) = self.in_block(block, |emit| {
            let frame_pointer = emit.take_frame_address(frame, frame_type);
            let progress = emit.push_resume_call(&coroutine, &frame_pointer, &args);
            (frame_pointer, progress)
        });

        PollLoop {
            resumed: coroutine,
            site: PollSite::Suspending(point),
            block,
            args,
            progress,
            frame_pointer,
        }
    }

    /// Reads the signature and the row layout of `instance`.
    async fn resumed_coroutine(&self, instance: MonoCoroutineInstance) -> ResumedCoroutine {
        let resolver = self.resolver();
        let signature = resolver.coroutine_signature(&instance).await;
        let row_layout = resolver.effect_row_layout(signature.row()).await;

        ResumedCoroutine { instance, signature, row_layout }
    }

    /// Resumes `coroutine` in the selected block, and returns the new local
    /// that holds what it returned.
    ///
    /// ```c
    ///     progress = callee_resume(frame_pointer, args);
    /// ```
    fn push_resume_call(
        &mut self,
        coroutine: &ResumedCoroutine,
        frame_pointer: &Place,
        args: &Place,
    ) -> Place {
        let progress = self.new_progress_temporary_place(&coroutine.signature);
        self.push_call(
            Call::builder()
                .destination(progress.clone())
                .callee(Operand::Function(MonoFunctionInstance::CoroutineResume(
                    coroutine.instance.clone(),
                )))
                .arguments(vec![Operand::Copy(frame_pointer.clone()), Operand::Copy(args.clone())])
                .build(),
        );

        progress
    }

    /// Stores the unit value that a frame that has not started is first
    /// resumed with in the `args` of `poll`.
    pub(super) fn store_start_resume_args(&mut self, poll: &PollLoop) {
        let unit = MonoType::new_unit(self.engine());
        self.store_constant(poll.resume_value_place(&unit, self.engine()), Constant::Unit);
    }

    /// Ends the current block by jumping to a new poll block, which it returns
    /// selected.
    ///
    /// At a suspension point, the state is stored before the first poll, so
    /// a forwarded operation needs nothing more to come back: the resume entry
    /// of the point converts the result of the operation and polls again.
    ///
    /// ```c
    ///     // in the current block:
    ///     frame->state = SUSPENDED_AT_p;           // suspending only
    ///     goto poll;
    ///
    /// resume_entry_p:                              // suspending only
    ///     args = <forwarded_resume_args>;
    ///     goto poll;
    ///
    /// poll:                                        // new, left selected
    /// ```
    fn enter_poll_block(
        &mut self,
        site: PollSite,
        row: &EffectRow,
        args: &Place,
        ctx: &Context<'_>,
    ) -> BlockID {
        let poll = self.create_block();

        if let PollSite::Suspending(point) = site {
            let forwarded_args = self.forwarded_resume_args(row, ctx);
            let destination = args.clone();
            self.fill_resume_entry(point, ctx, |emit| {
                emit.assign(destination, forwarded_args);
                emit.set_terminator(Terminator::Goto(poll));
            });
            self.store_suspended_state(point, ctx);
        }

        self.set_terminator(Terminator::Goto(poll));
        self.select_block(poll);

        poll
    }

    /// The `args` of this `coroutine_resume` as those of a coroutine of
    /// `row`, which performed the operation they hold the result of.
    ///
    /// The result is in a member of its type in both, but the row does not
    /// tell which member that is, so the conversion cannot name it. It does not
    /// have to: every member of a `coroutine_resume_t` starts at its first
    /// byte, so reading the bytes as the other type reads the same value.
    ///
    /// It emits nothing itself; it is the right-hand side of an assignment:
    ///
    /// ```c
    /// own_args                        // both rows are the same
    /// (callee_resume_t) own_args      // otherwise, a reinterpretation
    /// ```
    pub(super) fn forwarded_resume_args(&self, row: &EffectRow, ctx: &Context<'_>) -> Rvalue {
        let engine = self.engine();
        let form = ctx.coroutine_form();
        let args = Operand::Copy(form.resume_args_place().clone());
        let own_row = form.signature().row();

        if own_row == row {
            return Rvalue::Use(args);
        }

        Rvalue::Reinterpret(Reinterpret::new(
            args,
            resume_type(own_row, engine),
            resume_type(row, engine),
        ))
    }

    /// Stores the pointer to the frame of `resumee` in a new local of the
    /// selected block.
    ///
    /// ```c
    ///     frame_pointer = &frame;         // a new frame, held by value
    ///     frame_pointer = k.frame;        // a `core.Continuation` `k`
    /// ```
    async fn load_frame_pointer(&mut self, resumee: &Resumee) -> Place {
        let held = resumee.place().clone();
        match resumee {
            Resumee::NewFrame { frame_type, .. } => {
                self.take_frame_address(held, frame_type.clone())
            }
            Resumee::Continuation { continuation_type, .. } => {
                let (field, pointer_type) =
                    continuation_frame_field(self.resolver(), continuation_type).await;
                let frame_pointer = self.new_temporary_place(pointer_type);
                self.store(
                    frame_pointer.clone(),
                    Operand::Copy(self.project(&held).struct_field(field)),
                );
                frame_pointer
            }
        }
    }

    /// Stores the address of `frame`, of type `frame_type`, in a new local of
    /// the selected block.
    pub(super) fn take_frame_address(
        &mut self,
        frame: Place,
        frame_type: Interned<MonoType>,
    ) -> Place {
        let pointer_type = MonoType::new_pointer(frame_type, PointerMutability::Mut, self.engine());
        let frame_pointer = self.new_temporary_place(pointer_type);
        self.assign(
            frame_pointer.clone(),
            Rvalue::AddressOf(AddressOf::new(frame, PointerMutability::Mut)),
        );

        frame_pointer
    }

    /// Creates the arm taken when the coroutine completed, which stores its
    /// result in `destination` and continues in `done`.
    ///
    /// The result is stored only once the callee is done with its frame,
    /// which may share memory with `destination`.
    ///
    /// ```c
    /// return_arm:
    ///     destination = progress.return_value;
    ///     goto done;
    /// ```
    pub(super) fn return_arm(
        &mut self,
        poll: &PollLoop,
        destination: Place,
        done: BlockID,
    ) -> BlockID {
        let value = self.project(&poll.progress).coroutine_progress_return_payload();

        self.in_new_block(|emit| {
            emit.store(destination, Operand::Copy(value));
            emit.set_terminator(Terminator::Goto(done));
        })
    }

    /// The place of the arguments of `operation`, which the resumed coroutine
    /// yielded. It emits nothing; the place is:
    ///
    /// ```c
    /// progress.yield.operation
    /// ```
    pub(super) fn yielded_arguments(&self, poll: &PollLoop, operation: RowOperationID) -> Place {
        let yielded = self.project(&poll.progress).coroutine_progress_yield_payload();
        self.project(&yielded).coroutine_yield_operation(operation)
    }

    /// Terminates the poll block: a completed coroutine takes `return_arm`, an
    /// operation in `handled` takes its arm, and every other operation is
    /// forwarded.
    ///
    /// ```c
    /// poll:
    ///     ...                                      // from `begin_poll`
    ///     switch (progress.tag) {
    ///     case RETURN: goto return_arm;
    ///     case YIELD:  goto yield_arm;
    ///     }
    ///
    /// // with no clauses, in a coroutine whose progress has the callee's type:
    /// yield_arm:
    ///     return progress;
    ///
    /// // otherwise, `yield_arm` is the `yield_switch`
    /// ```
    pub(super) fn finish_poll(
        &mut self,
        poll: &PollLoop,
        return_arm: BlockID,
        handled: &[(RowOperationID, BlockID)],
        ctx: &Context<'_>,
    ) {
        // Without clauses, a coroutine whose progress has the same type as the
        // resumed one's passes on the whole progress.
        let yield_arm = if handled.is_empty() && poll.forwards_whole_progress(ctx) {
            self.forward_whole_progress_arm(poll)
        } else {
            self.yield_switch(poll, handled, ctx)
        };

        let tag = Operand::Copy(self.project(&poll.progress).coroutine_progress_tag());
        self.in_block(poll.block, |emit| {
            emit.set_switch_terminator(tag, vec![
                (CoroutineProgress::return_tag(), return_arm),
                (CoroutineProgress::yield_tag(), yield_arm),
            ]);
        });
    }

    /// Creates the block that branches on the operation the coroutine yielded.
    ///
    /// ```c
    /// yield_switch:
    ///     switch (progress.yield.tag) {
    ///     case OP_a: goto clause_arm_a;            // in `handled`
    ///     case OP_b: goto forward_arm_b;           // anything else; forward.rs
    ///     }
    /// ```
    fn yield_switch(
        &mut self,
        poll: &PollLoop,
        handled: &[(RowOperationID, BlockID)],
        ctx: &Context<'_>,
    ) -> BlockID {
        let row_layout = poll.row_layout();

        // Each operation goes to its clause, or else is forwarded.
        let mut targets = Vec::with_capacity(row_layout.row_operations().len());
        for operation in row_layout.row_operation_ids() {
            let arm = handled
                .iter()
                .find_map(|(handled, arm)| (*handled == operation).then_some(*arm))
                .unwrap_or_else(|| self.forward_arm(poll, operation, ctx));
            targets.push((row_layout.yield_tag(operation), arm));
        }

        let yielded = self.project(&poll.progress).coroutine_progress_yield_payload();
        let tag = Operand::Copy(self.project(&yielded).coroutine_yield_tag());
        self.in_new_block(|emit| emit.set_switch_terminator(tag, targets))
    }
}
