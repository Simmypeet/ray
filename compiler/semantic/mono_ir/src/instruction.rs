use qbice::{Decode, Encode, Identifiable, StableHash};

use crate::{operand::Operand, place::Place, rvalue::Rvalue};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Assign {
    destination: Place,
    value: Rvalue,
}

impl Assign {
    #[must_use]
    pub const fn new(destination: Place, value: Rvalue) -> Self { Self { destination, value } }

    #[must_use]
    pub const fn destination(&self) -> &Place { &self.destination }

    #[must_use]
    pub const fn value(&self) -> &Rvalue { &self.value }
}

/// A call at a precise position in a basic block.
///
/// A function operand denotes a direct call. Copying a function pointer from a
/// place denotes an indirect call. Calls returning `void` have no destination.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Call {
    destination: Option<Place>,
    callee: Operand,
    arguments: Vec<Operand>,
}

impl Call {
    #[must_use]
    pub const fn new(destination: Option<Place>, callee: Operand, arguments: Vec<Operand>) -> Self {
        Self { destination, callee, arguments }
    }

    #[must_use]
    pub const fn destination(&self) -> Option<&Place> { self.destination.as_ref() }

    #[must_use]
    pub const fn callee(&self) -> &Operand { &self.callee }

    #[must_use]
    pub fn arguments(&self) -> &[Operand] { &self.arguments }
}

/// An operation evaluated in sequence within a basic block.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Instruction {
    Assign(Assign),
    Call(Call),
}
