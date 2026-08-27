use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Literal {
    Numeric(u128),
    Bool(bool),
    String(Interned<str>),
}

impl SubExprs for Literal {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}
