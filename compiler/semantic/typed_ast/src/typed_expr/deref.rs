use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Deref {
    pointee: TypedExprID,
}

impl Deref {
    #[must_use]
    pub const fn new(pointee: TypedExprID) -> Self { Self { pointee } }
}
