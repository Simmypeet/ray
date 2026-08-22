use qbice::{Decode, Encode, StableHash};

use crate::expression::ExpressionID;

/// Projects an element from a tuple value.
///
/// Unlike an address projection, this operand need not denote a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct TupleIndex {
    operand: ExpressionID,
    index: usize,
}

impl TupleIndex {
    #[must_use]
    pub const fn new(operand: ExpressionID, index: usize) -> Self { Self { operand, index } }

    #[must_use]
    pub const fn operand(&self) -> ExpressionID { self.operand }

    #[must_use]
    pub const fn index(&self) -> usize { self.index }
}
