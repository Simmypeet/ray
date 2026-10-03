use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    Numeric(u128),
    /// The negation of a numeric literal, holding its magnitude, e.g. `128`
    /// in `-128i8`. The magnitude itself may not fit in the type.
    NegatedNumeric(u128),
    /// A floating-point literal, holding its digits as written in the source
    /// code.
    Float(Interned<str>),
    Bool(bool),
    String(Interned<str>),
}
