//! What a `coroutine_resume` leaves behind when it returns: the state of its
//! frame, and the `coroutine_progress_t` it returns.

use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_mono_ir::{
    cfg::Terminator,
    layout::coroutine::{FrameState, RowOperationKey},
    operand::{Constant, Operand},
    place::Place,
    ty::CoroutineProgress,
};

use crate::{context::Context, emitter::Emitter};

/// A return from `coroutine_resume` that yields one operation of its own row.
pub(in crate::lower) struct OwnYield {
    /// The progress the function returns.
    progress: Place,
    /// The tag of `progress`, which tells a yield from a return.
    progress_tag: Place,
    /// The tag of the yield payload, which tells the operations apart.
    operation_tag: Place,
    /// The value of `operation_tag` for the yielded operation.
    operation: Constant,
    /// The member of the yield payload that holds the arguments.
    arguments: Place,
}

/// The arguments of a yielded operation.
pub(in crate::lower) enum YieldArguments {
    /// One operand per argument, as a `perform` has them.
    Elements(Vec<Operand>),
    /// The whole argument tuple, as a resumed coroutine yielded it.
    Tuple(Operand),
}

impl Emitter<'_> {
    /// Ends the block `emit` has selected by returning the yield with
    /// `arguments`.
    pub(in crate::lower) fn emit_own_yield(
        &mut self,
        own_yield: OwnYield,
        arguments: YieldArguments,
    ) {
        self.store_constant(own_yield.progress_tag, CoroutineProgress::yield_tag());
        self.store_constant(own_yield.operation_tag, own_yield.operation);
        match arguments {
            YieldArguments::Elements(elements) => {
                self.initialize_tuple(&own_yield.arguments, elements);
            }
            YieldArguments::Tuple(tuple) => self.store(own_yield.arguments, tuple),
        }

        self.set_terminator(Terminator::Return(Some(Operand::Copy(own_yield.progress))));
    }

    /// Records in the frame that the coroutine is suspended at `point`, so
    /// that the next resume enters the resume entry of `point`.
    pub(in crate::lower) fn store_suspended_state(
        &mut self,
        point: SuspensionPointID,
        ctx: &Context<'_>,
    ) {
        let form = ctx.coroutine_form();
        let state = self.project(form.frame_place()).coroutine_frame_state();
        let suspended = form.frame_state_tag(FrameState::Suspended(point));

        self.store_constant(state, suspended);
    }

    /// A new local for the progress this coroutine returns.
    fn create_own_progress_temporary(&mut self, ctx: &Context<'_>) -> Place {
        let signature = ctx.coroutine_form().signature().clone();
        self.new_progress_temporary_place(&signature)
    }

    /// The yield of `key`, an operation of this coroutine's own row, built in
    /// a new progress local.
    pub(in crate::lower) fn create_own_yield(
        &mut self,
        key: &RowOperationKey,
        ctx: &Context<'_>,
    ) -> OwnYield {
        let progress = self.create_own_progress_temporary(ctx);
        let row_layout = ctx.coroutine_form().row_layout();
        let operation = row_layout.get_row_operation_id(key);

        let yielded = self.project(&progress).coroutine_progress_yield_payload();

        OwnYield {
            progress_tag: self.project(&progress).coroutine_progress_tag(),
            operation_tag: self.project(&yielded).coroutine_yield_tag(),
            operation: row_layout.yield_tag(operation),
            arguments: self.project(&yielded).coroutine_yield_operation(operation),
            progress,
        }
    }

    /// Lowers a `return` of a coroutine, which completes it.
    pub(in crate::lower) fn lower_complete_coroutine(&mut self, value: Operand, ctx: &Context<'_>) {
        let progress = self.create_own_progress_temporary(ctx);
        let form = ctx.coroutine_form();
        let state = self.project(form.frame_place()).coroutine_frame_state();
        let completed = form.frame_state_tag(FrameState::Completed);

        self.store_constant(state, completed);
        self.store_constant(
            self.project(&progress).coroutine_progress_tag(),
            CoroutineProgress::return_tag(),
        );
        self.store(self.project(&progress).coroutine_progress_return_payload(), value);
        self.set_terminator(Terminator::Return(Some(Operand::Copy(progress))));
    }
}
