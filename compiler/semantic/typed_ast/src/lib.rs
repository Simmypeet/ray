use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

use crate::typed_function::TypedFunctionMap;

pub mod block;
pub mod irrefutable_pattern;
pub mod name_binding;
pub mod statement;
pub mod typed_expr;
pub mod typed_function;
pub mod typed_lambda;
pub mod typed_variable;

/// Retrieves the typed AST for a given def ID.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<TypedFunctionMap>)]
#[extend(by_val, name = get_typed_ast)]
pub struct Key {
    pub def_id: GlobalSymbolID,
}
