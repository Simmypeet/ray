use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

/// Retrieves the effect row of a function symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Ty>)]
#[extend(by_val, name = get_effect_row)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
