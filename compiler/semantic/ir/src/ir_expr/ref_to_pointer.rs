use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

/// Coerces a reference value into a raw pointer to the same place.
///
/// The pointer type, including its mutability, is the type of the expression
/// itself. The memory behind the resulting pointer is no longer tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct RefToPointer {
    reference: IRExprID,
}

impl RefToPointer {
    #[must_use]
    pub const fn new(reference: IRExprID) -> Self { Self { reference } }

    /// Returns the reference value being coerced.
    #[must_use]
    pub const fn reference(&self) -> IRExprID { self.reference }
}
