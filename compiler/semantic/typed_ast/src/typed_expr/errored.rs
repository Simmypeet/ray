use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Errored {
    children: Vec<TypedExprID>,
}

impl Errored {
    #[must_use]
    pub const fn new_empty() -> Self { Self { children: Vec::new() } }

    #[must_use]
    pub const fn new(children: Vec<TypedExprID>) -> Self { Self { children } }
}
