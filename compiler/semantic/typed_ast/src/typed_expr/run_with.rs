use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct RunWith {
    effect: GlobalSymbolID,
}

impl RunWith {
    #[must_use]
    pub const fn new(effect: GlobalSymbolID) -> Self { Self { effect } }
}

impl SubExprs for RunWith {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}
