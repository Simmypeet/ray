use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::args::Args,
};

/// A reference to a trait together with its type arguments.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct TraitRef {
    trait_id: GlobalSymbolID,
    args: Args,
}

impl TraitRef {
    #[must_use]
    pub const fn new(trait_id: GlobalSymbolID, args: Args) -> Self { Self { trait_id, args } }

    #[must_use]
    pub const fn trait_id(&self) -> GlobalSymbolID { self.trait_id }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }

    /// Whether any argument recursively contains an inference variable.
    #[must_use]
    pub fn contains_inference(&self) -> bool { self.args.contains_inference() }

    /// Whether any argument recursively contains an error type.
    #[must_use]
    pub fn contains_error(&self) -> bool { self.args.contains_error() }
}

impl Reduce for TraitRef {
    async fn reduce(
        &self,
        engine: &rayc_qbice::TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<Self> {
        self.args.reduce(engine, givens).await.map(|args| Self::new(self.trait_id, args))
    }
}

impl Substitutable for TraitRef {
    fn apply_subst(&self, subst: &Subst, engine: &rayc_qbice::TrackedEngine) -> Option<Self> {
        self.args.apply_subst(subst, engine).map(|args| Self::new(self.trait_id, args))
    }
}
