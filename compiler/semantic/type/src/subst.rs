use im::hashmap::Entry;
use qbice::storage::intern::Interned;
use rayc_hash::FxImHashMap;
use rayc_qbice::TrackedEngine;

use crate::ty::{Ty, TyInference};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Subst(FxImHashMap<TyInference, Interned<Ty>>);

impl Subst {
    #[must_use]
    pub fn new_empty() -> Self { Self(FxImHashMap::default()) }

    #[must_use]
    pub fn new_singleton(inference: TyInference, ty: Interned<Ty>) -> Self {
        let mut map = FxImHashMap::default();
        map.insert(inference, ty);
        Self(map)
    }

    /// Composes this substitution with another substitution such that the
    /// updated substitution is equivalent to applying `self` followed by
    /// `other`.
    pub fn compose(&mut self, other: &Self, engine: &TrackedEngine) {
        for (_, ty) in self.0.iter_mut() {
            if let Some(new_ty) = ty.apply_subst(other, engine) {
                *ty = new_ty;
            }
        }

        for (inference, ty) in &other.0 {
            if let Entry::Vacant(e) = self.0.entry(*inference) {
                e.insert(ty.clone());
            }
        }
    }

    #[must_use]
    pub fn get(&self, inference: &TyInference) -> Option<&Interned<Ty>> { self.0.get(inference) }
}

pub trait Substitutable {
    #[must_use]
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized;

    fn apply_in_place(&mut self, subst: &Subst, engine: &TrackedEngine)
    where
        Self: Sized,
    {
        if let Some(new_self) = self.apply_subst(subst, engine) {
            *self = new_self;
        }
    }
}

impl FromIterator<(TyInference, Interned<Ty>)> for Subst {
    fn from_iter<T: IntoIterator<Item = (TyInference, Interned<Ty>)>>(iter: T) -> Self {
        Self(FxImHashMap::from_iter(iter))
    }
}

#[cfg(test)]
mod test;
