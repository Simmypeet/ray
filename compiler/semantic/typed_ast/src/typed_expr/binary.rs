use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOp {
    Plus,
    Minus,
    Multiply,
    Divide,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Binary {
    left: TypedExprID,
    operator: BinaryOp,
    right: TypedExprID,
}

impl Binary {
    #[must_use]
    pub const fn new(left: TypedExprID, operator: BinaryOp, right: TypedExprID) -> Self {
        Self { left, operator, right }
    }
}
