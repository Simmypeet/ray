//! Query for the symbols that the imports of a module name.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;

/// The names that the imports of a module bind, each to the symbol its path
/// names.
///
/// An import whose path could not be resolved binds nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default, StableHash, Encode, Decode, Identifiable)]
pub struct Imports {
    symbol_ids_by_name: FxHashMap<Interned<str>, GlobalSymbolID>,
}

impl Imports {
    /// Creates the imports that bind no name.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Binds `name` to `symbol_id`.
    ///
    /// The names of the imports of a module are distinct, so `name` must not
    /// be bound already.
    pub fn insert(&mut self, name: Interned<str>, symbol_id: GlobalSymbolID) {
        let previous = self.symbol_ids_by_name.insert(name, symbol_id);
        assert!(previous.is_none(), "the imports of a module should bind distinct names");
    }

    /// Returns the symbol that `name` is imported as, if any.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<GlobalSymbolID> {
        self.symbol_ids_by_name.get(name).copied()
    }

    /// Returns the imported names and the symbols they are bound to.
    pub fn iter(&self) -> impl Iterator<Item = (&Interned<str>, GlobalSymbolID)> {
        self.symbol_ids_by_name.iter().map(|(name, symbol_id)| (name, *symbol_id))
    }
}

/// Retrieves the symbols that the imports of a module name.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<Imports>)]
#[extend(by_val, name = get_imports)]
pub struct Key {
    /// The global ID of the module.
    pub module_id: GlobalSymbolID,
}
