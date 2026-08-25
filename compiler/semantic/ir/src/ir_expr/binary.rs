use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::ExpressionID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOp {
    Plus,
    Minus,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Binary {
    left: ExpressionID,
    operator: BinaryOp,
    right: ExpressionID,
}

impl Binary {
    #[must_use]
    pub const fn new(left: ExpressionID, operator: BinaryOp, right: ExpressionID) -> Self {
        Self { left, operator, right }
    }

    #[must_use]
    pub const fn left(&self) -> ExpressionID { self.left }

    #[must_use]
    pub const fn operator(&self) -> BinaryOp { self.operator }

    #[must_use]
    pub const fn right(&self) -> ExpressionID { self.right }
}
