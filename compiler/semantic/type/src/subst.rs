use im::hashmap::Entry;
use qbice::{
    Decode, Encode, StableHash,
    stable_hash::{StableHasher, Value},
    storage::intern::Interned,
};
use rayc_hash::FxImHashMap;
use rayc_qbice::TrackedEngine;

use crate::{
    fold::{TyFoldable, TyFolder},
    poly_var::GlobalPolyVarID,
    ty::{Ty, inference::Inference, lifetime::ExternalRegionID, self_instance::SelfInstance},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Var {
    Inference(Inference),
    Poly(GlobalPolyVarID),
    /// Substitutes a trait’s self dictionary when instantiating its members.
    SelfInstance(SelfInstance),
    /// Instantiates an external lifetime of a nested IR function.
    External(ExternalRegionID),
}

impl From<ExternalRegionID> for Var {
    fn from(external: ExternalRegionID) -> Self { Self::External(external) }
}

impl From<SelfInstance> for Var {
    fn from(instance: SelfInstance) -> Self { Self::SelfInstance(instance) }
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
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    #[must_use]
    pub fn has_inference_variable(&self, inference: &Inference) -> bool {
        self.0.contains_key(&Var::Inference(*inference))
    }

    pub fn insert(&mut self, var: impl Into<Var>, ty: Interned<Ty>) -> Option<Interned<Ty>> {
        self.0.insert(var.into(), ty)
    }

    #[must_use]
    pub fn codomain(&self) -> impl ExactSizeIterator<Item = &Interned<Ty>> { self.0.values() }

    /// Iterates mutably over the types this substitution maps to.
    pub fn codomain_mut(&mut self) -> impl Iterator<Item = &mut Interned<Ty>> {
        self.0.iter_mut().map(|(_, ty)| ty)
    }

    pub fn inference_mappings(&self) -> impl Iterator<Item = (Inference, &Interned<Ty>)> {
        self.0.iter().filter_map(|(var, ty)| match var {
            Var::Inference(inference) => Some((*inference, ty)),
            Var::Poly(_) | Var::SelfInstance(_) | Var::External(_) => None,
        })
    }

    /// Iterates over the polymorphic-variable mappings, in no stable order.
    pub fn poly_var_mappings(&self) -> impl Iterator<Item = (GlobalPolyVarID, &Interned<Ty>)> {
        self.0.iter().filter_map(|(var, ty)| match var {
            Var::Poly(poly_var) => Some((*poly_var, ty)),
            Var::External(_) | Var::Inference(_) | Var::SelfInstance(_) => None,
        })
    }
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

    /// Composes `self` with `other`, as if applying `self` and then `other`.
    pub async fn compose(&mut self, other: &Self, engine: &TrackedEngine) {
        for (_, ty) in self.0.iter_mut() {
            if let Some(new_ty) = ty.apply_subst(other, engine).await {
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

    /// Erases the lifetimes in every type this substitution maps to; see
    /// [`Ty::erase_lifetimes`].
    pub async fn erase_lifetimes(&mut self, engine: &TrackedEngine) {
        for (_, ty) in self.0.iter_mut() {
            *ty = Ty::erase_lifetimes(ty, engine).await;
        }
    }
}

/// Instantiates the types in a value by a [`Subst`].
///
/// Every [`TyFoldable`] value is substitutable; rebuilding goes through the
/// smart constructors, so a substitution may reduce what it made reducible.
pub trait Substitutable: TyFoldable {
    #[must_use]
    #[allow(async_fn_in_trait)]
    async fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        self.fold_with(&mut SubstFolder { subst }, engine).await
    }

    #[must_use]
    #[allow(async_fn_in_trait)]
    async fn apply_subst_or_clone(&self, subst: &Subst, engine: &TrackedEngine) -> Self
    where
        Self: Clone,
    {
        self.apply_subst(subst, engine).await.unwrap_or_else(|| self.clone())
    }

    #[allow(async_fn_in_trait)]
    async fn apply_in_place(&mut self, subst: &Subst, engine: &TrackedEngine) {
        if let Some(new_self) = self.apply_subst(subst, engine).await {
            *self = new_self;
        }
    }
}

impl<T: TyFoldable> Substitutable for T {}

/// The [`TyFolder`] behind [`Substitutable`], which replaces the variables
/// that `subst` maps.
struct SubstFolder<'a> {
    subst: &'a Subst,
}

impl TyFolder for SubstFolder<'_> {
    async fn fold_ty(&mut self, ty: &Interned<Ty>, engine: &TrackedEngine) -> Option<Interned<Ty>> {
        match ty.as_var() {
            Some(var) => self.subst.0.get(&var).cloned(),
            None => Ty::super_fold_with(ty, self, engine).await,
        }
    }
}

pub trait MutSubstitutable {
    #[allow(async_fn_in_trait)]
    async fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine);
}

impl MutSubstitutable for Subst {
    async fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for (_, ty) in self.0.iter_mut() {
            ty.apply_in_place(subst, engine).await;
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
