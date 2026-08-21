use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Tuple {
    elements: Vec<TypedExprID>,
}

impl Tuple {
    #[must_use]
    pub const fn new(elements: Vec<TypedExprID>) -> Self { Self { elements } }

    #[must_use]
    pub fn elements(&self) -> &[TypedExprID] { &self.elements }
}
