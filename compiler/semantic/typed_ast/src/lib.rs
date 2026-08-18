use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::function::Function;

pub mod block;
pub mod function;
pub mod irrefutable_pattern;
pub mod name_binding;
pub mod statement;
pub mod typed_expr;
pub mod variable;

/// Retrieves the typed AST for a given def ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Function>)]
#[extend(by_val, name = get_typed_ast)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
