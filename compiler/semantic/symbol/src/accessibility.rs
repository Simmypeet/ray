//! Contains the definition of [`Accessibility`], which determines from where a
//! symbol can be referred to.

use qbice::{Decode, Encode, Identifiable, Query, StableHash};
use rayc_qbice::TrackedEngine;

use crate::{
    GlobalSymbolID,
    parent::{HierarchyRelationship, symbol_hierarchy_relationship},
};

/// Determines from where a symbol can be referred to.
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
pub enum Accessibility {
    /// The symbol is accessible from anywhere, declared with `pub`.
    Public,

    /// The symbol is accessible only from the given symbol and its
    /// descendants.
    ///
    /// A declaration without an access modifier is scoped to the module
    /// declaring it.
    Scoped(GlobalSymbolID),
}

impl Accessibility {
    /// Checks whether a symbol with this accessibility can be referred to from
    /// `site`.
    pub async fn is_accessible_from(&self, site: GlobalSymbolID, engine: &TrackedEngine) -> bool {
        match self {
            Self::Public => true,
            Self::Scoped(scope) => {
                // a scope never spans across targets
                if scope.target_id != site.target_id {
                    return false;
                }

                matches!(
                    engine.symbol_hierarchy_relationship(site.target_id, site.id, scope.id).await,
                    HierarchyRelationship::Child | HierarchyRelationship::Equivalent
                )
            }
        }
    }
}

/// The accessibility a symbol is declared with.
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
pub enum DeclaredAccessibility {
    /// The symbol has its own accessibility.
    Declared(Accessibility),

    /// The symbol is an instance member, which can't have an access modifier
    /// and has the accessibility of the trait member it implements.
    InheritedFromTraitMember,
}

/// The key type used with [`TrackedEngine`] to access the accessibility a
/// symbol is declared with.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(DeclaredAccessibility)]
#[extend(name = get_declared_accessibility, by_val)]
pub struct Key {
    /// The global ID of the symbol to get the accessibility for.
    pub symbol_id: GlobalSymbolID,
}
