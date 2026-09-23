use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

/// Moves out of the operand, even when its type is `Copy`.
///
/// Moving an operand which is not an lvalue has no effect, since its value is
/// a fresh temporary already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Move {
    operand: TypedExprID,
}

impl Move {
    #[must_use]
    pub const fn new(operand: TypedExprID) -> Self { Self { operand } }

    #[must_use]
    pub const fn operand(&self) -> TypedExprID { self.operand }
}

impl SubExprs for Move {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.operand) }
}
