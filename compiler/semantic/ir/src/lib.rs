pub mod address;
pub mod cfg;
pub mod ir_expr;
pub mod ir_function;
pub mod ir_lambda;
pub mod ir_variable;
pub mod visit;

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::ir_function::IRFunctionMap;

/// Retrieves the completed control-flow IR functions for a source function
/// definition.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<IRFunctionMap>)]
#[extend(by_val, name = get_ir)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
