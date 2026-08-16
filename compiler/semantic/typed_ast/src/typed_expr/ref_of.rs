use qbice::{Decode, Encode, StableHash};
use rayc_type::ty::Mutability;

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct RefOf {
    pointee: TypedExprID,
    mutability: Mutability,
}

impl RefOf {
    #[must_use]
    pub const fn new(pointee: TypedExprID, mutability: Mutability) -> Self {
        Self { pointee, mutability }
    }

    #[must_use]
    pub const fn mutability(&self) -> Mutability { self.mutability }
}
