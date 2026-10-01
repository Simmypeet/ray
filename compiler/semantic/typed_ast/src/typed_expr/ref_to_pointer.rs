use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

/// Coerces a reference into a raw pointer to the same place, as a `&t` value
/// used where a `*t` is expected.
///
/// The pointer type, including its mutability, is the type of the expression
/// itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct RefToPointer {
    reference: TypedExprID,
}

impl RefToPointer {
    #[must_use]
    pub const fn new(reference: TypedExprID) -> Self { Self { reference } }

    /// Returns the expression of reference type being coerced.
    #[must_use]
    pub const fn reference(&self) -> TypedExprID { self.reference }
}

impl SubExprs for RefToPointer {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.reference) }
}
