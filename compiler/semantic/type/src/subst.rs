use im::hashmap::Entry;
use qbice::storage::intern::Interned;
use rayc_hash::FxImHashMap;
use rayc_qbice::TrackedEngine;

use crate::{
    poly_var::GlobalPolyVarID,
    ty::{Ty, TyInference},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Var {
    Inference(TyInference),
    Poly(GlobalPolyVarID),
}

impl From<TyInference> for Var {
    fn from(inference: TyInference) -> Self { Self::Inference(inference) }
}

impl From<GlobalPolyVarID> for Var {
    fn from(poly: GlobalPolyVarID) -> Self { Self::Poly(poly) }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Subst(FxImHashMap<Var, Interned<Ty>>);

impl Subst {
    #[must_use]
    pub fn new_empty() -> Self { Self(FxImHashMap::default()) }

    #[must_use]
    pub fn new_singleton<V: Into<Var>>(var: V, ty: Interned<Ty>) -> Self {
        let mut map = FxImHashMap::default();
        map.insert(var.into(), ty);
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

        for (var, ty) in &other.0 {
            if let Entry::Vacant(e) = self.0.entry(*var) {
                e.insert(ty.clone());
            }
        }
    }

    #[must_use]
    pub fn get<V: Copy + Into<Var>>(&self, var: &V) -> Option<&Interned<Ty>> {
        self.0.get(&(*var).into())
    }
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

pub trait MutSubstitutable {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine);
}

impl<V> FromIterator<(V, Interned<Ty>)> for Subst
where
    V: Into<Var>,
{
    fn from_iter<T: IntoIterator<Item = (V, Interned<Ty>)>>(iter: T) -> Self {
        Self(iter.into_iter().map(|(var, ty)| (var.into(), ty)).collect())
    }
}

#[cfg(test)]
mod test;
