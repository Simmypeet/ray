//! Checks the trait implemented by an explicitly supplied instance.

use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    fold::{TyFoldable, TyFolder, fold_pair},
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

impl TyFoldable for InstanceTraitRef {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        fold_pair(&self.instance, &self.expected, folder, engine)
            .await
            .map(|(instance, expected)| Self::new(instance, expected))
    }
}
