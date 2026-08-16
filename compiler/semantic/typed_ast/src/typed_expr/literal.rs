use qbice::{Decode, Encode, StableHash};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    Numeric(u128),
    Bool(bool),
}
