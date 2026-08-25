use qbice::{Decode, Encode, StableHash};

use crate::ir_expr::IRExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Tuple {
    elements: Vec<IRExprID>,
}

impl Tuple {
    #[must_use]
    pub const fn new(elements: Vec<IRExprID>) -> Self { Self { elements } }

    #[must_use]
    pub fn elements(&self) -> &[IRExprID] { &self.elements }
}
