use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Return {
    value: TypedExprID,
}

impl Return {
    #[must_use]
    pub const fn new(value: TypedExprID) -> Self { Self { value } }

    #[must_use]
    pub const fn value(&self) -> TypedExprID { self.value }
}
