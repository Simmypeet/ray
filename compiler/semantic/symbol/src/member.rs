//! Contains the definition of [`Member`] type.

use std::collections::hash_map::Entry;

use rayc_extend::extend;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_qbice::TrackedEngine;
use rayc_target::Global;
use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};

use crate::{SymbolID, symbol_kind::get_symbol_kind};

/// Stores the members of a symbol in a form of `::Member`
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode, StableHash, Identifiable)]
pub struct Member {
    /// A map from the member name to its ID.
    ///
    /// In case of the redefinition, the firs encounter is recorded in this
    /// map. The redefinition is recorded in the [`Self::unnameds`] field.
    member_ids_by_name: FxHashMap<Interned<str>, SymbolID>,

    /// A set of members that doesn't have a name associated to it.
    ///
    /// These IDs could be redefinitions or implements item the module.
    unnameds: FxHashSet<SymbolID>,
}

/// Results of inserting a new member
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash)]
pub enum Insertion {
    /// Successfully inserted the member.
    Inserted,

    /// The member is a redefinition of an existing member.
    Conflicted(SymbolID),
}

impl Member {
    /// Retrieves the member ID by its name.
    ///
    /// Returns `None` if there is no member with the given name.   
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<SymbolID> {
        self.member_ids_by_name.get(name).copied()
    }

    /// Retrieves all the member IDs of the symbol.
    pub fn all_ids(&self) -> impl Iterator<Item = SymbolID> + '_ {
        self.member_ids_by_name.values().copied().chain(self.unnameds.iter().copied())
    }

    /// Inserts a new symbol ID with the given name into the member.
    #[must_use]
    pub fn insert(&mut self, name: Interned<str>, id: SymbolID) -> Insertion {
        match self.member_ids_by_name.entry(name) {
            Entry::Occupied(entry) => {
                self.unnameds.insert(id);
                Insertion::Conflicted(*entry.get())
            }
            Entry::Vacant(entry) => {
                entry.insert(id);
                Insertion::Inserted
            }
        }
    }
}

/// Retrieves the member ID of the given name in the symbol with the given ID.
#[extend]
pub async fn get_member_by_name(
    self: &TrackedEngine,
    symbol_id: Global<SymbolID>,
    name: &str,
) -> Option<Global<SymbolID>> {
    let members = self.get_members(symbol_id).await;
    members.get_by_name(name).map(|id| symbol_id.target_id.make_global(id))
}

/// The key type used with [`TrackedEngine`] to access the members of a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<Member>)]
#[extend(name = get_members, by_val)]
pub struct Key {
    /// The global ID of the symbol to get the members for.
    pub symbol_id: Global<SymbolID>,
}

/// Optionally returns `None` if the given symbol is of a kind that does not
/// have members.
#[extend]
pub async fn try_get_members(
    self: &TrackedEngine,
    id: Global<SymbolID>,
) -> Option<Interned<Member>> {
    if !self.get_symbol_kind(id).await.has_member() {
        return None;
    }

    Some(self.get_members(id).await)
}
