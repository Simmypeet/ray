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
mod test {
    use super::*;
    use crate::ty::{Mutability, Primitive, TyKind};

    // input: {T0 -> T1} composed with {T1 -> int32}
    // premise: {}
    // output: {T0 -> int32, T1 -> int32}
    #[tokio::test]
    async fn compose_rewrites_existing_bindings() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let t0 = TyInference::new(TyKind::Star, 0);
        let t1 = TyInference::new(TyKind::Star, 1);
        let t1_ty = engine.intern(Ty::Inference(t1));
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut subst = Subst::new_singleton(t0, t1_ty);
        let other = Subst::new_singleton(t1, int32.clone());

        subst.compose(&other, &engine);

        assert_eq!(subst.0.len(), 2);
        assert_eq!(subst.get(&t0), Some(&int32));
        assert_eq!(subst.get(&t1), Some(&int32));
    }

    // input: {T0 -> T1} composed with {T0 -> int32}
    // premise: both substitutions bind T0
    // output: {T0 -> T1}
    #[tokio::test]
    async fn compose_preserves_first_substitution_domain_precedence() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let t0 = TyInference::new(TyKind::Star, 0);
        let t1 = TyInference::new(TyKind::Star, 1);
        let t1_ty = engine.intern(Ty::Inference(t1));
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let mut subst = Subst::new_singleton(t0, t1_ty.clone());
        let other = Subst::new_singleton(t0, int32);

        subst.compose(&other, &engine);

        assert_eq!(subst.0.len(), 1);
        assert_eq!(subst.get(&t0), Some(&t1_ty));
    }

    // input: {T0 -> *mut T1} composed with {T1 -> int32}
    // premise: {}
    // output: {T0 -> *mut int32, T1 -> int32}
    #[tokio::test]
    async fn compose_rewrites_nested_types() {
        let engine = rayc_qbice::create_minimal_engine().await;
        let t0 = TyInference::new(TyKind::Star, 0);
        let t1 = TyInference::new(TyKind::Star, 1);
        let t1_ty = engine.intern(Ty::Inference(t1));
        let pointer_to_t1 = Ty::new_pointer(t1_ty, Mutability::Mutable, &engine);
        let int32 = Ty::new_primitive(Primitive::Int32, &engine);
        let pointer_to_int32 = Ty::new_pointer(int32.clone(), Mutability::Mutable, &engine);
        let mut subst = Subst::new_singleton(t0, pointer_to_t1);
        let other = Subst::new_singleton(t1, int32.clone());

        subst.compose(&other, &engine);

        assert_eq!(subst.0.len(), 2);
        assert_eq!(subst.get(&t0), Some(&pointer_to_int32));
        assert_eq!(subst.get(&t1), Some(&int32));
    }
}
