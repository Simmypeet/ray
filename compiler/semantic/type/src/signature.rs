//! Function signature queries, declared here because coroutine reduction in
//! [`crate::reduce`] reads them.

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::ty::Ty;

/// Retrieves the return type of a function symbol
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Ty>)]
#[extend(by_val, name = get_return_type)]
pub struct ReturnTypeKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the effect row of a function symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Ty>)]
#[extend(by_val, name = get_effect_row)]
pub struct EffectRowKey {
    pub symbol_id: GlobalSymbolID,
}
