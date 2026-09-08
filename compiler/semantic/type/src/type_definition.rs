//! Queries for the resolved definitions of instance associated types.

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::ty::Ty;

/// Retrieves an `InstanceType` definition in its own polymorphic scope,
/// before substituting enclosing instance and associated-type arguments.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Ty>)]
#[extend(by_val, name = get_type_definition)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
