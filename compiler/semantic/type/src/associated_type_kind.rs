//! Declared result kinds of trait associated types.

use qbice::{Decode, Encode, Query, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::ty::TyKind;

/// Retrieves the result kind of a trait associated type, defaulting to `Star`.
/// This query depends on the declaration, never on an instance definition.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(TyKind)]
#[extend(by_val, name = get_associated_type_kind)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
