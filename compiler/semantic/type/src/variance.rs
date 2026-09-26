//! The variance of type constructor parameters.
//!
//! Variance is the four-point lattice with [`Variance::Bivariant`] at the
//! bottom, [`Variance::Invariant`] at the top, and the covariant and
//! contravariant points in between. The variances of built-in constructors are
//! fixed. Those of struct and `eff` parameters are computed from their
//! declarations; see [`VarianceKey`].

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;

use crate::{
    poly_var::{PolyVarID, PolyVarMap},
    ty::TyKind,
};

/// How a relation between two instantiations of a type constructor relates
/// the arguments at one parameter.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum Variance {
    /// The parameter is unused: its arguments are never related.
    Bivariant,

    /// `a <: b` relates the arguments as `a_arg <: b_arg`.
    Covariant,

    /// `a <: b` relates the arguments as `b_arg <: a_arg`.
    Contravariant,

    /// The arguments are related by equality.
    Invariant,
}

impl Variance {
    /// Returns the variance of a position that has variance `inner` inside a
    /// position of variance `self`.
    ///
    /// Covariant keeps the inner variance, contravariant flips it, and an
    /// invariant or bivariant context absorbs it. This is rustc's `xform`, so
    /// `Invariant.xform(Bivariant)` is `Invariant`.
    #[must_use]
    pub const fn xform(self, inner: Self) -> Self {
        match self {
            Self::Covariant => inner,
            Self::Contravariant => inner.flip(),
            Self::Invariant => Self::Invariant,
            Self::Bivariant => Self::Bivariant,
        }
    }

    /// Returns the least variance that is at least as strong as both, which
    /// is the variance of a parameter that occurs at both positions.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Bivariant, variance) | (variance, Self::Bivariant) => variance,
            (Self::Covariant, Self::Covariant) => Self::Covariant,
            (Self::Contravariant, Self::Contravariant) => Self::Contravariant,
            (Self::Invariant, _)
            | (_, Self::Invariant)
            | (Self::Covariant, Self::Contravariant)
            | (Self::Contravariant, Self::Covariant) => Self::Invariant,
        }
    }

    /// Swaps covariance and contravariance.
    #[must_use]
    pub const fn flip(self) -> Self {
        match self {
            Self::Covariant => Self::Contravariant,
            Self::Contravariant => Self::Covariant,
            Self::Invariant => Self::Invariant,
            Self::Bivariant => Self::Bivariant,
        }
    }
}

/// The variance of every polymorphic variable of a struct or `eff`.
///
/// The variances are kept in the order of the owner's [`PolyVarMap`], which is
/// also the order of its arguments, and can be looked up by poly var ID too.
#[derive(Debug, Clone, PartialEq, Eq, Default, StableHash, Encode, Decode, Identifiable)]
pub struct VarianceMap {
    variances: Vec<Variance>,
    indices_by_id: FxHashMap<PolyVarID, usize>,
}

impl VarianceMap {
    /// Creates a map in which every polymorphic variable of `poly_vars` is
    /// [`Variance::Bivariant`], that is, not used yet.
    #[must_use]
    pub fn new_unused(poly_vars: &PolyVarMap) -> Self {
        Self {
            variances: vec![Variance::Bivariant; poly_vars.len()],
            indices_by_id: poly_vars
                .iter()
                .enumerate()
                .map(|(index, (id, _))| (id, index))
                .collect(),
        }
    }

    /// Returns the variances in the order of the poly var map.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Variance> + '_ {
        self.variances.iter().copied()
    }

    /// Returns the variance of the polymorphic variable with the given ID.
    ///
    /// # Panics
    ///
    /// If the ID is not in the poly var map this one was created from.
    #[must_use]
    pub fn get(&self, id: PolyVarID) -> Variance { self.variances[self.index_of(id)] }

    /// Returns the variance of the argument at `index`.
    ///
    /// # Panics
    ///
    /// If `index` is not less than the number of polymorphic variables.
    #[must_use]
    pub fn get_by_index(&self, index: usize) -> Variance { self.variances[index] }

    /// Joins `variance` into the variance of the polymorphic variable with
    /// the given ID, recording one more use of it. Returns whether its
    /// variance changed.
    ///
    /// # Panics
    ///
    /// If the ID is not in the poly var map this one was created from.
    pub fn join(&mut self, id: PolyVarID, variance: Variance) -> bool {
        let index = self.index_of(id);
        let joined = self.variances[index].join(variance);
        let changed = joined != self.variances[index];
        self.variances[index] = joined;
        changed
    }

    /// Returns the position of the polymorphic variable with the given ID.
    fn index_of(&self, id: PolyVarID) -> usize {
        *self.indices_by_id.get(&id).expect("the poly var belongs to this map")
    }

    /// Makes every unused polymorphic variable that is not a lifetime
    /// invariant, so that no `PhantomData` equivalent is needed. An unused
    /// lifetime stays bivariant, to be reported. `poly_vars` must be the map
    /// this one was created from. Returns whether any variance changed.
    pub fn default_unused(&mut self, poly_vars: &PolyVarMap) -> bool {
        let mut changed = false;
        for (variance, (_, poly_var)) in self.variances.iter_mut().zip(poly_vars.iter()) {
            if *variance == Variance::Bivariant && poly_var.kind() != TyKind::Lifetime {
                *variance = Variance::Invariant;
                changed = true;
            }
        }
        changed
    }
}

/// Retrieves the variance of every polymorphic variable of a struct or an
/// `eff`, in the order of its poly var map, which is also the order of its
/// arguments.
///
/// A struct parameter's variance is the join of its uses in the field types.
/// A `perform` calls the handler, so an `eff` parameter used in an
/// operation's parameter types is covariant, one used in its return type is
/// contravariant, and one used in both is invariant.
///
/// An unused lifetime parameter is [`Variance::Bivariant`] and is reported as
/// an error. Every other unused parameter is [`Variance::Invariant`], so no
/// `PhantomData` equivalent is needed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<VarianceMap>)]
#[extend(by_val, name = get_variance)]
pub struct VarianceKey {
    /// A symbol whose kind has a variance map; see
    /// `SymbolKind::has_variance_map`.
    pub symbol_id: GlobalSymbolID,
}
