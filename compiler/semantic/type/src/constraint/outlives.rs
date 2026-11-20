//! Outlives constraints between lifetimes, emitted by relating and reducing
//! types since relating lifetimes never binds anything.

use qbice::{
    Decode, Encode, StableHash,
    stable_hash::{StableHasher, Value},
    storage::intern::Interned,
};
use rayc_hash::FxImHashSet;
use rayc_qbice::TrackedEngine;

use crate::{
    fold::{TyFoldable, TyFolder},
    ty::{Ty, lifetime::Lifetime},
    variance::Variance,
    where_clause::OutlivesPredicate,
};

impl OutlivesPredicate {
    /// Returns the predicates that relating `lesser` to `greater` with
    /// `variance` requires, leaving out those that trivially hold.
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

        forward.into_iter().chain(backward).filter(|predicate| !predicate.is_trivial())
    }

    /// Returns whether this predicate between two lifetimes holds whatever
    /// the lifetimes are; see [`Self::from_relation`].
    fn is_trivial(&self) -> bool {
        self.lesser() == self.greater()
            || **self.lesser() == Ty::Lifetime(Lifetime::Static)
            || !self.lesser().is_checked_lifetime()
            || !self.greater().is_checked_lifetime()
    }
}

/// An immutable, structurally shared set of outlives predicates, so combining
/// the constraints of several steps is cheap.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OutlivesConstraints(FxImHashSet<OutlivesPredicate>);

impl OutlivesConstraints {
    /// Creates an empty set.
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Returns the constraints of relating `lesser` to `greater`; see
    /// [`OutlivesPredicate::from_relation`].
    #[must_use]
    pub fn from_relation(
        lesser: &Interned<Ty>,
        greater: &Interned<Ty>,
        variance: Variance,
    ) -> Self {
        OutlivesPredicate::from_relation(lesser, greater, variance).collect()
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
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &OutlivesPredicate> { self.0.iter() }
}

impl FromIterator<OutlivesPredicate> for OutlivesConstraints {
    fn from_iter<I: IntoIterator<Item = OutlivesPredicate>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl TyFoldable for OutlivesConstraints {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        let mut changed = false;
        let mut constraints = FxImHashSet::default();
        for constraint in &self.0 {
            match constraint.fold_with(folder, engine).await {
                Some(constraint) => {
                    changed = true;
                    constraints.insert(constraint);
                }
                None => {
                    constraints.insert(constraint.clone());
                }
            }
        }
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
            constraints.insert(OutlivesPredicate::decode(decoder, plugin, session)?);
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
