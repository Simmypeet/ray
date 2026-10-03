use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum UnaryOp {
    /// Negates a signed numeric operand, e.g. `-x`.
    Negate,
}

/// Applies a prefix operator to its operand. The result has the operand's
/// type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Unary {
    operator: UnaryOp,
    operand: TypedExprID,
}

impl Unary {
    #[must_use]
    pub const fn new(operator: UnaryOp, operand: TypedExprID) -> Self { Self { operator, operand } }

    #[must_use]
    pub const fn operator(&self) -> UnaryOp { self.operator }

    #[must_use]
    pub const fn operand(&self) -> TypedExprID { self.operand }
}

impl SubExprs for Unary {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.operand) }
}
