pub mod address;
pub mod cfg;
pub mod expression;
pub mod function;
pub mod lambda;
pub mod variable;
pub mod visit;

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::function::FunctionMap;

/// Retrieves the completed control-flow IR functions for a source function
/// definition.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FunctionMap>)]
#[extend(by_val, name = get_ir)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
