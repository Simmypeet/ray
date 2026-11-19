use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
};

/// The rigid `this` dictionary of a trait, which keeps projections like
/// `this.Item` abstract until a member is selected through an instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct SelfInstance(GlobalSymbolID);

impl SelfInstance {
    /// Creates a self binder. `trait_id` must identify a `SymbolKind::Trait`.
    #[must_use]
    pub const fn new(trait_id: GlobalSymbolID) -> Self { Self(trait_id) }

    /// Returns the owning trait applied to its identity poly variables.
    pub async fn trait_ref(self, engine: &TrackedEngine) -> TraitRef {
        let poly_vars = engine.get_poly_var_map(self.0).await;
        let args = Args::new(
            poly_vars
                .iter()
                .map(|(id, _)| engine.intern(Ty::PolyVar(GlobalPolyVarID::new(self.0, id)))),
            engine,
        );
        TraitRef::new(self.0, args)
    }
}
