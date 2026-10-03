use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum UnaryOp {
    Negate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Unary {
    operator: UnaryOp,
    operand: IRExprID,
}

impl Unary {
    #[must_use]
    pub const fn new(operator: UnaryOp, operand: IRExprID) -> Self { Self { operator, operand } }

    #[must_use]
    pub const fn operator(&self) -> UnaryOp { self.operator }

    #[must_use]
    pub const fn operand(&self) -> IRExprID { self.operand }
}
