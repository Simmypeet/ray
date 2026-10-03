use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

/// Converts a numeric operand to another numeric type, e.g. `x as int64`.
///
/// The target type is the type of the cast expression itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Cast {
    operand: TypedExprID,
}

impl Cast {
    #[must_use]
    pub const fn new(operand: TypedExprID) -> Self { Self { operand } }

    #[must_use]
    pub const fn operand(&self) -> TypedExprID { self.operand }
}

impl SubExprs for Cast {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.operand) }
}
