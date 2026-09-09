use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
};

/// The rigid self-instance binder owned by a trait declaration.
///
/// The following Ray declaration uses this binder as the dictionary in the
/// associated type projection `this.Item`:
///
/// ```text
/// trait Iterator[a]:
///     type Item
///     def next(value: a, previous: this.Item) -> (bool, this.Item)
/// ```
///
/// Its kind is `TyKind::Instance`, and its trait reference is `Iterator[a]`,
/// using the owning trait's identity poly variables. Selecting `next` through
/// instance `i` substitutes this binder with `i`, yielding `(bool, i.Item)`.
/// Until then, the projection remains abstract.
///
/// `this` is scoped to trait bodies, including their member signatures. It is
/// not valid in instance declarations or ordinary function bodies. Bare `this`
/// can supply an explicit given dictionary inside a trait signature; it is not
/// a value type, runtime value, or implicit instance-search request. Named
/// trait paths such as `Iterator[int32].Item` provide no dictionary and are
/// rejected.
///
/// This binder lives in the trait's original parameter context. Instantiating
/// a member must substitute both the trait parameters and this binder;
/// replacing only the parameters cannot specialize the trait reference stored
/// implicitly by this trait ID. It is not an additional ordinary trait
/// parameter.
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
