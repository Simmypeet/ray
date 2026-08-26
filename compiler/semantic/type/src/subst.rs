use im::hashmap::Entry;
use qbice::{
    Decode, Encode, Identifiable, StableHash,
    stable_hash::{StableHasher, Value},
    storage::intern::Interned,
};
use rayc_hash::FxImHashMap;
use rayc_qbice::TrackedEngine;

use crate::{
    poly_var::GlobalPolyVarID,
    ty::{Ty, inference::Inference},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Var {
    Inference(Inference),
    Poly(GlobalPolyVarID),
}

impl From<Inference> for Var {
    fn from(inference: Inference) -> Self { Self::Inference(inference) }
}

impl From<GlobalPolyVarID> for Var {
    fn from(poly: GlobalPolyVarID) -> Self { Self::Poly(poly) }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Subst(FxImHashMap<Var, Interned<Ty>>);

impl Subst {
    #[must_use]
    pub fn codomain(&self) -> impl ExactSizeIterator<Item = &Interned<Ty>> { self.0.values() }
}

impl Encode for Subst {
    fn encode<E: qbice::serialize::Encoder + ?Sized>(
        &self,
        encoder: &mut E,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<()> {
        encoder.emit_usize(self.0.len())?;

        for (var, ty) in &self.0 {
            var.encode(encoder, plugin, session)?;
            ty.encode(encoder, plugin, session)?;
        }

        Ok(())
    }
}

impl Decode for Subst {
    fn decode<D: qbice::serialize::Decoder + ?Sized>(
        decoder: &mut D,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<Self> {
        let len = decoder.read_usize()?;
        let mut map = FxImHashMap::default();

        for _ in 0..len {
            let var = Var::decode(decoder, plugin, session)?;
            let ty = Interned::decode(decoder, plugin, session)?;
            map.insert(var, ty);
        }

        Ok(Self(map))
    }
}

impl StableHash for Subst {
    fn stable_hash<H: StableHasher + ?Sized>(&self, state: &mut H) {
        self.0.len().stable_hash(state);
        let mut combined = H::Hash::default();

        for (var, ty) in &self.0 {
            combined = combined.wrapping_add(state.sub_hash(&mut |sub| {
                var.stable_hash(sub);
                ty.stable_hash(sub);
            }));
        }

        combined.stable_hash(state);
    }
}

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

    #[must_use]
    fn apply_subst_or_clone(&self, subst: &Subst, engine: &TrackedEngine) -> Self
    where
        Self: Sized + Clone,
    {
        self.apply_subst(subst, engine).unwrap_or_else(|| self.clone())
    }

    fn apply_in_place(&mut self, subst: &Subst, engine: &TrackedEngine)
    where
        Self: Sized,
    {
        if let Some(new_self) = self.apply_subst(subst, engine) {
            *self = new_self;
        }
    }
}

impl<T: Substitutable + Clone + StableHash + Identifiable + Send + Sync + 'static> Substitutable
    for Interned<[T]>
{
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        let mut new_vec = None;

        for (i, ty_arg) in self.as_ref().iter().enumerate() {
            match (new_vec.as_mut(), ty_arg.apply_subst(subst, engine)) {
                (None, Some(new_ty_arg)) => {
                    let mut vec = self.as_ref().to_vec();
                    vec[i] = new_ty_arg;
                    new_vec = Some(vec);
                }
                (Some(vec), Some(new_ty_arg)) => {
                    vec[i] = new_ty_arg;
                }
                _ => {}
            }
        }

        new_vec.map(|vec| engine.intern_unsized(vec))
    }
}

pub trait MutSubstitutable {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine);
}

impl MutSubstitutable for Subst {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for (_, ty) in self.0.iter_mut() {
            ty.apply_in_place(subst, engine);
        }
    }
}

impl<V> FromIterator<(V, Interned<Ty>)> for Subst
where
    V: Into<Var>,
{
    fn from_iter<T: IntoIterator<Item = (V, Interned<Ty>)>>(iter: T) -> Self {
        Self(iter.into_iter().map(|(var, ty)| (var.into(), ty)).collect())
    }
}
