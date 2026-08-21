use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct TupleIndex {
    operand: TypedExprID,
    index: usize,
}

impl TupleIndex {
    #[must_use]
    pub const fn new(operand: TypedExprID, index: usize) -> Self { Self { operand, index } }

    #[must_use]
    pub const fn index(&self) -> usize { self.index }

    #[must_use]
    pub const fn operand(&self) -> TypedExprID { self.operand }
}
