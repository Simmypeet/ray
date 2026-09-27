//! Checks the trait implemented by an explicitly supplied instance.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::outlives::OutlivesConstraints,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::Ty,
};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct InstanceTraitRef {
    instance: Interned<Ty>,
    expected: TraitRef,
}

impl InstanceTraitRef {
    #[must_use]
    pub const fn new(instance: Interned<Ty>, expected: TraitRef) -> Self {
        Self { instance, expected }
    }

    #[must_use]
    pub const fn instance(&self) -> &Interned<Ty> { &self.instance }

    #[must_use]
    pub const fn expected(&self) -> &TraitRef { &self.expected }

    pub fn interned_recursive_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        Ty::interned_recursive_iter(&self.instance)
            .chain(self.expected.args().interned_iter().flat_map(Ty::interned_recursive_iter))
    }
}

impl Reduce for InstanceTraitRef {
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
    ) -> Option<(Self, OutlivesConstraints)> {
        let instance = self.instance.reduce(engine, givens).await;
        let expected = self.expected.reduce(engine, givens).await;
        if instance.is_none() && expected.is_none() {
            return None;
        }

        let (instance, instance_outlives) =
            instance.unwrap_or_else(|| (self.instance.clone(), OutlivesConstraints::new()));
        let (expected, expected_outlives) =
            expected.unwrap_or_else(|| (self.expected.clone(), OutlivesConstraints::new()));
        Some((Self::new(instance, expected), instance_outlives.union(expected_outlives)))
    }
}

impl Substitutable for InstanceTraitRef {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        match (self.instance.apply_subst(subst, engine), self.expected.apply_subst(subst, engine)) {
            (None, None) => None,
            (instance, expected) => Some(Self::new(
                instance.unwrap_or_else(|| self.instance.clone()),
                expected.unwrap_or_else(|| self.expected.clone()),
            )),
        }
    }
}
