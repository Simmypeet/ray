use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;

use crate::expression::Expression;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum BinaryOp {
    Plus,
    Minus,
    Multiply,
    Divide,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Binary {
    left: ID<Expression>,
    operator: BinaryOp,
    right: ID<Expression>,
}
