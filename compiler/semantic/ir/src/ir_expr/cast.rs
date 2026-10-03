use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

/// Converts a numeric operand to the numeric type of the cast expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Cast {
    operand: IRExprID,
}

impl Cast {
    #[must_use]
    pub const fn new(operand: IRExprID) -> Self { Self { operand } }

    #[must_use]
    pub const fn operand(&self) -> IRExprID { self.operand }
}
