use derive_more::From;
use qbice::{Decode, Encode, StableHash};
use rayc_ir::{address::Local, cfg::Instruction, ir_expr::IRExprID};

use crate::suspension_point::SuspensionPointID;

/// The values that can be saved into a coroutine frame
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, From,
)]
pub enum Savable {
    Local(Local),
    Expression(IRExprID),

    /// What the function waits for while suspended at the suspension point:
    /// the callee's frame of a call, the frames of the handled body and its
    /// handlers of a `run ... with`, or the continuation of a forwarding
    /// `handle`. A `perform` has none.
    Awaitee(SuspensionPointID),
}

impl Savable {
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
