use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

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

impl SubExprs for TupleIndex {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.operand) }
}
