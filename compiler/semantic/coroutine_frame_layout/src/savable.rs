use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_ir::{address::Local, cfg::Instruction, ir_expr::IRExprID};

use crate::suspension_point::SuspensionPointID;

/// The values that can be saved into a coroutine frame
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
    From,
)]
pub enum Savable {
    Local(Local),
    Expression(IRExprID),

    /// The child frame of the suspension point: the coroutine that the
    /// function owns and waits for while suspended there. It is the callee's
    /// frame of a call, or the frames of the handled body and its handlers of a
    /// `run ... with`.
    ///
    /// It has no counterpart in the IR. The lowering creates it when the
    /// function reaches the suspension point, and it is gone once the
    /// function leaves it, whether the child completes or is abandoned.
    ///
    /// A `perform` has none. Neither has a forwarding `handle` or a call of a
    /// continuation: the continuation only points to a frame that something
    /// else owns, so its operand is held as an ordinary
    /// [`Savable::Expression`] instead.
    ChildFrame(SuspensionPointID),

    /// The pointer to the environment of a function that borrows its
    /// captures, which is an operation handler.
    ///
    /// The clauses of a `run ... with` share one environment, which each of
    /// their frames points to. So such a frame never holds a capture, only
    /// this pointer, from which every capture is read.
    Environment,
}

impl Savable {
    /// Whether this is the child frame of a suspension point.
    #[must_use]
    pub const fn is_child_frame(&self) -> bool {
        match self {
            Self::ChildFrame(_) => true,
            Self::Local(_) | Self::Expression(_) | Self::Environment => false,
        }
    }

    /// Returns the value whose own storage `instruction` writes, if any.
    ///
    /// This is read from the instruction, not from liveness: a store to a
    /// part of a local writes its storage without ending its value, and the
    /// end of a scope ends the values of its variables without writing them.
    #[must_use]
    pub fn written_by(instruction: &Instruction) -> Option<Self> {
        match instruction {
            Instruction::Expression(expression_id) => Some(Self::Expression(*expression_id)),

            // A store through a dereference writes the memory behind the
            // pointer, not the local holding it.
            Instruction::Store(store) => store.address().direct_local().map(Self::Local),

            Instruction::ScopePush(_)
            | Instruction::ScopePop(_)
            | Instruction::ExprDiscard(_)
            | Instruction::AddressDrop(_) => None,
        }
    }
}
