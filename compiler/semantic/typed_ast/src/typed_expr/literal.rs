use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    /// An integer literal such as `23`, `23i8`, or `-23`; a negated literal
    /// is a single negative literal. Its type may still be a floating-point
    /// type, e.g. `23f32`.
    Numeric(i128),
    /// A floating-point literal such as `1.5` or `-1.5`, holding its digits
    /// as written in the source code, with its sign but without a suffix.
    Float(Interned<str>),
    Bool(bool),
    String(Interned<str>),
}

impl SubExprs for Literal {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}
