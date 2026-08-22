pub mod address;
pub mod cfg;
pub mod expression;
pub mod function;
pub mod variable;

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::function::Function;

/// Retrieves the completed control-flow IR for a function definition.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Function>)]
#[extend(by_val, name = get_ir)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
