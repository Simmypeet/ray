use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    Numeric(i128),
    /// A floating-point literal, holding its digits as written in the source
    /// code.
    Float(Interned<str>),
    Bool(bool),
    String(Interned<str>),
}
