use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOp {
    Equal,
    NotEqual,
    Plus,
    Minus,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Binary {
    left: IRExprID,
    operator: BinaryOp,
    right: IRExprID,
}

impl Binary {
    #[must_use]
    pub const fn new(left: IRExprID, operator: BinaryOp, right: IRExprID) -> Self {
        Self { left, operator, right }
    }

    #[must_use]
    pub const fn left(&self) -> IRExprID { self.left }

    #[must_use]
    pub const fn operator(&self) -> BinaryOp { self.operator }

    #[must_use]
    pub const fn right(&self) -> IRExprID { self.right }
}
