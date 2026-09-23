//! Tracked identities of declarations understood by the compiler.
use qbice::{Decode, Encode, Query, StableHash};

use crate::GlobalSymbolID;

/// Closed set of roles in the bundled core contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash)]
pub enum CoreItem {
    DefTrait,
    DefCall,
    DefArgs,
    DefReturn,
    DefEffect,
    Copy,
    DropTrait,
    DropMethod,
    NoDropStruct,
}

/// Finds an actual, unambiguous core declaration without checking signatures.
///
/// Panics if the compiler bundle is missing the declaration, redefines it, or
/// gives it the wrong symbol kind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(GlobalSymbolID)]
#[extend(name = get_core_item, by_val)]
pub struct Key {
    pub role: CoreItem,
}
