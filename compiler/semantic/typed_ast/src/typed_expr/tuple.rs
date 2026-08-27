use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

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

impl SubExprs for Tuple {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { self.elements.iter().copied() }
}
