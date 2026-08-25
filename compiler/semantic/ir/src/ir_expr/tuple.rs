use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::ExpressionID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Tuple {
    elements: Vec<ExpressionID>,
}

impl Tuple {
    #[must_use]
    pub const fn new(elements: Vec<ExpressionID>) -> Self { Self { elements } }

    #[must_use]
    pub fn elements(&self) -> &[ExpressionID] { &self.elements }
}
