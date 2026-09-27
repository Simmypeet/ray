use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    constraint::outlives::OutlivesSink,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::Ty,
    variance::Variance,
};

/// Relating two types together with a variance.
///
/// [`Variance::Covariant`] means `lesser <: greater`,
/// [`Variance::Contravariant`] means `greater <: lesser`, and
/// [`Variance::Invariant`] means equality. Ray has no subtyping apart from
/// lifetimes, so the structure of the two types is always unified exactly,
/// whatever the variance. The variance only decides which outlives
/// constraints relating two lifetimes produces.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct TyRelate {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
    variance: Variance,
}

impl TyRelate {
    pub fn interned_recursive_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        Ty::interned_recursive_iter(&self.lesser).chain(Ty::interned_recursive_iter(&self.greater))
    }

    #[must_use]
    pub const fn lesser(&self) -> &Interned<Ty> { &self.lesser }

    #[must_use]
    pub const fn greater(&self) -> &Interned<Ty> { &self.greater }

    #[must_use]
    pub const fn variance(&self) -> Variance { self.variance }

    #[must_use]
    pub const fn new(lesser: Interned<Ty>, greater: Interned<Ty>, variance: Variance) -> Self {
        Self { lesser, greater, variance }
    }

    /// Creates the equality `left = right`.
    #[must_use]
    pub const fn new_invariant(left: Interned<Ty>, right: Interned<Ty>) -> Self {
        Self::new(left, right, Variance::Invariant)
    }
}

impl Reduce for TyRelate {
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[crate::where_clause::PredicateKind],
        outlives: &mut OutlivesSink,
    ) -> Option<Self>
    where
        Self: Sized,
    {
        match (
            self.lesser.reduce(engine, givens, outlives).await,
            self.greater.reduce(engine, givens, outlives).await,
        ) {
            (None, None) => None,
            (lesser, greater) => Some(Self {
                lesser: lesser.unwrap_or_else(|| self.lesser.clone()),
                greater: greater.unwrap_or_else(|| self.greater.clone()),
                variance: self.variance,
            }),
        }
    }
}

impl Substitutable for TyRelate {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (self.lesser.apply_subst(subst, engine), self.greater.apply_subst(subst, engine)) {
            (None, None) => None,
            (lesser, greater) => Some(Self {
                lesser: lesser.unwrap_or_else(|| self.lesser.clone()),
                greater: greater.unwrap_or_else(|| self.greater.clone()),
                variance: self.variance,
            }),
        }
    }
}
