//! Queries for the declared signature of a function symbol.
//!
//! They are declared here, rather than next to the other semantic elements,
//! because reducing the associated types of a coroutine type reads them; see
//! [`crate::reduce`].

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
