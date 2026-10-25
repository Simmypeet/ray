//! Query for the resolved payload types of enum variants.

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

/// Retrieves the resolved payload type of a variant symbol, such as `T` in
/// `Some(T)`, or `None` for a variant declared without a payload.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Interned<Ty>>)]
#[extend(by_val, name = get_variant_payload_ty)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
