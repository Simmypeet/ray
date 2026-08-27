use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Deref {
    pointee: TypedExprID,
}

impl Deref {
    #[must_use]
    pub const fn new(pointee: TypedExprID) -> Self { Self { pointee } }

    #[must_use]
    pub const fn pointee(&self) -> TypedExprID { self.pointee }
}

impl SubExprs for Deref {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.pointee) }
}
