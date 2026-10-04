//! Determines the accessibility of symbols, including instance members, whose
//! accessibility is inherited from the trait members they implement.

use rayc_extend::extend;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{
    GlobalSymbolID,
    accessibility::{Accessibility, DeclaredAccessibility, get_declared_accessibility},
    member::get_member_by_name,
    name::get_name,
    parent::get_parent_global,
};

use crate::trait_ref::get_instance_trait_ref;

/// Returns the accessibility of the given symbol.
///
/// An instance member has the accessibility of the trait member it
/// implements. An instance member implementing no trait member, which is
/// already reported, has the accessibility of its instance.
#[extend]
pub async fn get_accessibility(self: &TrackedEngine, symbol_id: GlobalSymbolID) -> Accessibility {
    match self.get_declared_accessibility(symbol_id).await {
        DeclaredAccessibility::Declared(accessibility) => accessibility,
        DeclaredAccessibility::InheritedFromTraitMember => {
            let instance_id = self
                .get_parent_global(symbol_id)
                .await
                .expect("an instance member should have a parent instance");

            // The trait member is found by name rather than through the
            // instance member's correspondence, which resolves the member's
            // signature and could refer back to this symbol.
            let trait_member_id = match self.get_instance_trait_ref(instance_id).await {
                Some(trait_ref) => {
                    let name = self.get_name(symbol_id).await;
                    self.get_member_by_name(trait_ref.trait_id(), &name).await
                }
                None => None,
            };

            match self.get_declared_accessibility(trait_member_id.unwrap_or(instance_id)).await {
                DeclaredAccessibility::Declared(accessibility) => accessibility,
                DeclaredAccessibility::InheritedFromTraitMember => {
                    unreachable!("trait members and instances declare their own accessibility")
                }
            }
        }
    }
}

/// Checks whether the given symbol can be referred to from `site`.
#[extend]
pub async fn is_symbol_accessible_from(
    self: &TrackedEngine,
    symbol_id: GlobalSymbolID,
    site: GlobalSymbolID,
) -> bool {
    self.get_accessibility(symbol_id).await.is_accessible_from(site, self).await
}
