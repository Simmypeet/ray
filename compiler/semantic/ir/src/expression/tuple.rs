use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;

use crate::expression::Expression;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Tuple {
    args: Vec<ID<Expression>>,
}

impl Tuple {
    #[must_use]
    pub const fn new(args: Vec<ID<Expression>>) -> Self { Self { args } }
}
