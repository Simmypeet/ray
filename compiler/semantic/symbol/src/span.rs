//! Contains the queries for the spans of a symbol: the span of its name,
//! and the span of its whole definition.

use derive_more::{Deref, DerefMut};
use qbice::{Decode, Encode, Query, StableHash};
use rayc_lexical::tree::RelativeSpan;
use rayc_target::Global;

use crate::SymbolID;

/// Points to the particular location span in the source code where the given
/// symbol is defined, typically used for diagnostics and error reporting.
///
/// It usually points to the identifier of the symbol.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deref,
    DerefMut,
    Encode,
    Decode,
    Query,
    StableHash,
)]
#[extend(name = get_span, by_val)]
#[value(Option<RelativeSpan>)]
pub struct Key {
    /// The global ID of the symbol to get the span for.
    pub symbol_id: Global<SymbolID>,
}

/// Retrieves the span of the whole definition of a symbol, such as a `def`
/// from its keyword to the end of its body.
///
/// A symbol defined inside another, such as a struct in a module or a method
/// in a trait, has its definition inside the definition of its parent. A
/// file module, and the root module of a target, is defined by the contents
/// of its file. Returns `None` for a module whose file could not be loaded.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Deref,
    DerefMut,
    Encode,
    Decode,
    Query,
    StableHash,
)]
#[extend(name = get_definition_span, by_val)]
#[value(Option<RelativeSpan>)]
pub struct DefinitionKey {
    /// The global ID of the symbol to get the definition span for.
    pub symbol_id: Global<SymbolID>,
}
