//! Outlives constraints between two lifetimes, produced as a side output of
//! relating and reducing types.
//!
//! Relating two lifetimes never binds anything. Instead, it emits outlives
//! constraints `lesser: greater`, which the borrow checker keeps and type
//! inference drops; see [`OutlivesSink`].

use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
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

/// Collects the outlives constraints produced while relating or reducing
/// types, or drops them.
///
/// Type inference ignores lifetimes, so it drops every constraint. The IR type
/// check keeps them and tags each one with the point where it holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutlivesSink {
    /// `None` when the constraints are dropped.
    constraints: Option<Vec<OutlivesConstraint>>,
}

impl OutlivesSink {
    /// Creates a sink that drops every constraint.
    #[must_use]
    pub const fn dropping() -> Self { Self { constraints: None } }

    /// Creates a sink that keeps every constraint.
    #[must_use]
    pub const fn keeping() -> Self { Self { constraints: Some(Vec::new()) } }

    /// Returns whether this sink keeps the constraints pushed into it.
    #[must_use]
    pub const fn is_keeping(&self) -> bool { self.constraints.is_some() }

    /// Adds constraints to this sink, or drops them.
    pub fn extend(&mut self, constraints: impl IntoIterator<Item = OutlivesConstraint>) {
        if let Some(kept) = &mut self.constraints {
            kept.extend(constraints);
        }
    }

    /// Adds the constraints that relating the lifetime `lesser` to the
    /// lifetime `greater` with `variance` requires; see
    /// [`OutlivesConstraint::from_relation`].
    pub fn relate(&mut self, lesser: &Interned<Ty>, greater: &Interned<Ty>, variance: Variance) {
        if self.is_keeping() {
            self.extend(OutlivesConstraint::from_relation(lesser, greater, variance));
        }
    }

    /// Returns the kept constraints in the order they were added, or nothing
    /// if this sink drops them.
    #[must_use]
    pub fn into_constraints(self) -> Vec<OutlivesConstraint> {
        self.constraints.unwrap_or_default()
    }
}
