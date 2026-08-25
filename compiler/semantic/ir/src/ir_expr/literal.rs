use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    Numeric(u128),
    Bool(bool),
    String(Interned<str>),
}
