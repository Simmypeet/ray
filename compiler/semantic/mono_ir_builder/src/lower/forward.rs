//! Passes on an operation that a resumed coroutine performed and the function
//! being lowered has no clause for.
//!
//! A `coroutine_resume` at a suspension point yields it as one of its own, so
//! it is re-entered at the resume entry with the result, and polls again. Any
//! other function has the handler of the operation at hand, so it calls it
//! and polls again with its result.

use rayc_mono_ir::{
    cfg::{BlockID, Terminator},
    layout::coroutine::RowOperationID,
    operand::Operand,
    place::FieldIndex,
};

use super::{
    coroutine::YieldArguments,
    poll::{PollLoop, PollSite},
};
use crate::function_lowering::FunctionLowering;

/// How the function being lowered passes on an operation.
enum Forwarding {
    /// Returns it from `coroutine_resume` as a yield of its own row.
    YieldOut,
    /// Calls its handler, and polls again with the result.
    CallHandler,
}

impl FunctionLowering<'_> {
    /// How this function forwards an operation from a poll loop at `site`.
    fn forwarding(&self, site: PollSite) -> Forwarding {
        match (self.is_coroutine(), site) {
            (true, PollSite::Suspending(_)) => Forwarding::YieldOut,
            (false, PollSite::Inline) => Forwarding::CallHandler,
            (true, PollSite::Inline) => panic!(
                "a coroutine forwards an operation only from a suspension point: the frame layout \
                 suspends wherever an operation escapes"
            ),
            (false, PollSite::Suspending(_)) => {
                unreachable!("only a `coroutine_resume` has suspension points")
            }
        }
    }

    /// Creates the arm that forwards `operation`, which the resumed coroutine
    /// performed.
    pub(super) fn forward_arm(&mut self, poll: &PollLoop, operation: RowOperationID) -> BlockID {
        match self.forwarding(poll.site()) {
            Forwarding::YieldOut => self.yield_out_arm(poll, operation),
            Forwarding::CallHandler => self.call_handler_arm(poll, operation),
        }
    }

    /// Whether this function forwards every operation by returning the
    /// progress of the resumed coroutine as it is, which it can when the two
    /// progress types are the same.
    pub(super) fn forwards_whole_progress(&self, poll: &PollLoop) -> bool {
        // Only a suspending loop is in a `coroutine_resume` that returns its
        // operations.
        poll.site().is_suspending() && poll.has_progress_type_of(self.coroutine_form().signature())
    }

    /// Creates the arm that returns the progress of the resumed coroutine as
    /// this coroutine's own.
    pub(super) fn forward_whole_progress_arm(&mut self, poll: &PollLoop) -> BlockID {
        let progress = Operand::Copy(poll.progress().clone());
        self.in_new_block(|emit| emit.set_terminator(Terminator::Return(Some(progress))))
    }

    /// Creates the arm that yields `operation` in the terms of this
    /// coroutine's own row. Its position and tag may differ, but its arguments
    /// have the same type: both rows name the same effect instance.
    fn yield_out_arm(&mut self, poll: &PollLoop, operation: RowOperationID) -> BlockID {
        let own_yield = self.own_yield(poll.row_layout()[operation].key());
        let arguments = Operand::Copy(self.yielded_arguments(poll, operation));
        self.in_new_block(|x| {
            x.emit_own_yield(own_yield, YieldArguments::Tuple(arguments));
        })
    }

    /// Creates the arm that calls this function's handler of `operation`,
    /// and polls again with its result.
    fn call_handler_arm(&mut self, poll: &PollLoop, operation: RowOperationID) -> BlockID {
        let row_layout = poll.row_layout();
        let row_operation = &row_layout[operation];
        let key = row_operation.key();
        let (callee, environment) =
            self.handler_operation_callback(key.effect(), key.operation_id());

        // The handler ABI takes the environment, then the expanded arguments.
        let tuple = self.yielded_arguments(poll, operation);
        let mut arguments = vec![environment];
        arguments.extend((0..row_operation.arguments().assert_as_tuple().len()).map(|index| {
            Operand::Copy(self.project(&tuple).tuple_field(FieldIndex::from_usize(index)))
        }));

        // The result goes in the member of `args` of its type.
        let member = row_layout.get_resume_variant_id(row_operation.resume_type());
        let destination = self.project(poll.args()).coroutine_resume_value(member);
        let poll_block = poll.block();

        self.in_new_block(|emit| {
            emit.call(Some(destination), callee, arguments);
            emit.set_terminator(Terminator::Goto(poll_block));
        })
    }
}
