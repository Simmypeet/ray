use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;

use crate::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::Ty,
};

/// Relating two types together.
///
/// This can be used to implement various flavors of type relations, such as
/// subtyping, equality, unification, and one-way unification.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct TyRelate {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
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
    pub const fn new(lesser: Interned<Ty>, greater: Interned<Ty>) -> Self {
        Self { lesser, greater }
    }
}

impl Reduce for TyRelate {
    async fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (self.lesser.reduce(engine).await, self.greater.reduce(engine).await) {
            (Some(lesser), Some(greater)) => Some(Self { lesser, greater }),
            (Some(lesser), None) => Some(Self { lesser, greater: self.greater.clone() }),
            (None, Some(greater)) => Some(Self { lesser: self.lesser.clone(), greater }),
            (None, None) => None,
        }
    }
}

impl Substitutable for TyRelate {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        match (self.lesser.apply_subst(subst, engine), self.greater.apply_subst(subst, engine)) {
            (Some(lesser), Some(greater)) => Some(Self { lesser, greater }),
            (Some(lesser), None) => Some(Self { lesser, greater: self.greater.clone() }),
            (None, Some(greater)) => Some(Self { lesser: self.lesser.clone(), greater }),
            (None, None) => None,
        }
    }
}
