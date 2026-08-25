//!  Contains the definition of the [`Kind`] enum.

use std::{fmt::Debug, hash::Hash, sync::Arc};

use qbice::{Decode, Encode, Identifiable, Query, StableHash};
use rayc_target::{Global, TargetID};

use crate::SymbolID;

/// An enumeration used to identify the kind of a symbol in the Ray. This
/// value should be set to every symbol that is defined in the compilation
/// target.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    Identifiable,
)]
#[allow(missing_docs)]
pub enum SymbolKind {
    Def,
    ExternDef,
    Module,
}

/// The key type used with [`TrackedEngine`] to access the kind of a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, Query, StableHash,
)]
#[value(SymbolKind)]
#[extend(name = get_symbol_kind, by_val)]
pub struct Key {
    /// The global ID of the symbol to get the kind for.
    pub symbol_id: Global<SymbolID>,
}

impl SymbolKind {
    /// Checks if this kind of symbol has a [`Member`] component.
    #[must_use]
    pub const fn has_member(&self) -> bool { matches!(self, Self::Module) }

    /// Returns the human-readable string representation of this symbol kind.
    #[must_use]
    pub const fn str(&self) -> &'static str {
        match self {
            Self::Def => "def",
            Self::ExternDef => "extern def",
            Self::Module => "module",
        }
    }
}

/// A query that returns all the symbol IDs in a given target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_symbol_ids, by_val)]
pub struct AllSymbolIDs {
    pub target: TargetID,
}

/// Retrieves all the def symbol IDs in a given target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_def_ids, by_val)]
pub struct AllDefIDs {
    pub target: TargetID,
}

/// Retrieves all ordinary and extern callable definition IDs in a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_callable_def_ids, by_val)]
pub struct AllCallableDefIDs {
    pub target: TargetID,
}
