//! Outlives constraints between two lifetimes, produced as a side output of
//! relating and reducing types.
//!
//! Relating two lifetimes never binds anything. Instead, it emits outlives
//! constraints `lesser: greater`, which the borrow checker keeps and type
//! inference drops; see [`OutlivesConstraints`].

use qbice::{
    Decode, Encode, StableHash,
    stable_hash::{StableHasher, Value},
    storage::intern::Interned,
};
use rayc_hash::FxImHashSet;
use rayc_qbice::TrackedEngine;

use crate::{
    subst::{Subst, Substitutable},
    ty::{Ty, lifetime::Lifetime},
    variance::Variance,
};

/// The requirement that the lifetime `lesser` outlives the lifetime
/// `greater`, as in `'lesser: 'greater`.
///
/// The names follow [`TyRelate`](crate::constraint::ty_relate::TyRelate): a
/// lifetime that outlives another is its subtype, so `'lesser: 'greater`
/// holds exactly when `'lesser <: 'greater`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct OutlivesConstraint {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
}

impl OutlivesConstraint {
    /// Creates the constraint `lesser: greater`. Both must be of kind
    /// [`TyKind::Lifetime`](crate::ty::TyKind::Lifetime).
    #[must_use]
    pub const fn new(lesser: Interned<Ty>, greater: Interned<Ty>) -> Self {
        Self { lesser, greater }
    }

    /// Returns the lifetime that must outlive the other one.
    #[must_use]
    pub const fn lesser(&self) -> &Interned<Ty> { &self.lesser }

    /// Returns the lifetime that the other one must outlive.
    #[must_use]
    pub const fn greater(&self) -> &Interned<Ty> { &self.greater }

    /// Returns the constraints that relating the lifetime `lesser` to the
    /// lifetime `greater` with `variance` requires.
    ///
    /// Covariant `'a <: 'b` requires `'a: 'b`, contravariant requires
    /// `'b: 'a`, invariant requires both, and bivariant requires nothing.
    /// Constraints that always hold are left out: those between a lifetime
    /// and itself, those whose lesser lifetime is `'static`, and those that
    /// mention an erased lifetime or an error, since erased lifetimes are
    /// checked on the IR and errors were already reported.
    pub fn from_relation(
        lesser: &Interned<Ty>,
        greater: &Interned<Ty>,
        variance: Variance,
    ) -> impl Iterator<Item = Self> {
        let (forward, backward) = match variance {
            Variance::Bivariant => (false, false),
            Variance::Covariant => (true, false),
            Variance::Contravariant => (false, true),
            Variance::Invariant => (true, true),
        };

        let forward = forward.then(|| Self::new(lesser.clone(), greater.clone()));
        let backward = backward.then(|| Self::new(greater.clone(), lesser.clone()));

        forward.into_iter().chain(backward).filter(|constraint| !constraint.is_trivial())
    }

    /// Returns whether this constraint holds whatever the lifetimes are; see
    /// [`Self::from_relation`].
    fn is_trivial(&self) -> bool {
        self.lesser == self.greater
            || *self.lesser == Ty::Lifetime(Lifetime::Static)
            || !self.lesser.is_checked_lifetime()
            || !self.greater.is_checked_lifetime()
    }
}

impl Substitutable for OutlivesConstraint {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        match (self.lesser.apply_subst(subst, engine), self.greater.apply_subst(subst, engine)) {
            (None, None) => None,
            (lesser, greater) => Some(Self::new(
                lesser.unwrap_or_else(|| self.lesser.clone()),
                greater.unwrap_or_else(|| self.greater.clone()),
            )),
        }
    }
}

/// A set of outlives constraints.
///
/// The set is immutable and shares its structure, so combining the
/// constraints of several steps with [`Self::union`] is cheap.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OutlivesConstraints(FxImHashSet<OutlivesConstraint>);

impl OutlivesConstraints {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Returns the constraints that relating the lifetime `lesser` to the
    /// lifetime `greater` with `variance` requires; see
    /// [`OutlivesConstraint::from_relation`].
    #[must_use]
    pub fn from_relation(
        lesser: &Interned<Ty>,
        greater: &Interned<Ty>,
        variance: Variance,
    ) -> Self {
        OutlivesConstraint::from_relation(lesser, greater, variance).collect()
    }

    /// Returns the constraints of both sets.
    #[must_use]
    pub fn union(self, other: Self) -> Self { Self(self.0.union(other.0)) }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.0.is_empty() }

    #[must_use]
    pub fn len(&self) -> usize { self.0.len() }

    /// Returns the constraints in an unspecified order.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &OutlivesConstraint> { self.0.iter() }
}

impl FromIterator<OutlivesConstraint> for OutlivesConstraints {
    fn from_iter<I: IntoIterator<Item = OutlivesConstraint>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Substitutable for OutlivesConstraints {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        let mut changed = false;
        let constraints = self
            .0
            .iter()
            .map(|constraint| {
                constraint.apply_subst(subst, engine).map_or_else(
                    || constraint.clone(),
                    |substituted| {
                        changed = true;
                        substituted
                    },
                )
            })
            .collect();
        changed.then_some(Self(constraints))
    }
}

impl Encode for OutlivesConstraints {
    fn encode<E: qbice::serialize::Encoder + ?Sized>(
        &self,
        encoder: &mut E,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<()> {
        encoder.emit_usize(self.0.len())?;
        for constraint in &self.0 {
            constraint.encode(encoder, plugin, session)?;
        }
        Ok(())
    }
}

impl Decode for OutlivesConstraints {
    fn decode<D: qbice::serialize::Decoder + ?Sized>(
        decoder: &mut D,
        plugin: &qbice::serialize::Plugin,
        session: &mut qbice::serialize::session::Session,
    ) -> std::io::Result<Self> {
        let len = decoder.read_usize()?;
        let mut constraints = FxImHashSet::default();
        for _ in 0..len {
            constraints.insert(OutlivesConstraint::decode(decoder, plugin, session)?);
        }
        Ok(Self(constraints))
    }
}

impl StableHash for OutlivesConstraints {
    fn stable_hash<H: StableHasher + ?Sized>(&self, state: &mut H) {
        // The iteration order of the set is unspecified, so the hashes of the
        // constraints are combined commutatively.
        self.0.len().stable_hash(state);
        let mut combined = H::Hash::default();
        for constraint in &self.0 {
            combined =
                combined.wrapping_add(state.sub_hash(&mut |sub| constraint.stable_hash(sub)));
        }
        combined.stable_hash(state);
    }
}
