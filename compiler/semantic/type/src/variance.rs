//! The variance of type constructor parameters, used to relate their
//! arguments soundly.

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
    /// Returns the variance of a position with variance `inner` nested inside a
    /// position of variance `self` (rustc's `xform`).
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

    /// Returns whether `self` is at most `bound` in the lattice, so that a
    /// parameter used at `self` may be declared as `bound`.
    #[must_use]
    pub fn is_within(self, bound: Self) -> bool {
        // The derived order is not the lattice order, but `join` is the least
        // upper bound.
        self.join(bound) == bound
    }

    /// Returns the name of the variance, as in "covariant".
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bivariant => "bivariant",
            Self::Covariant => "covariant",
            Self::Contravariant => "contravariant",
            Self::Invariant => "invariant",
        }
    }

    /// Returns the marker that declares the variance before a parameter, as
    /// the `+` of `+t`, or `None` for bivariance, which cannot be declared.
    #[must_use]
    pub const fn marker(self) -> Option<&'static str> {
        match self {
            Self::Bivariant => None,
            Self::Covariant => Some("+"),
            Self::Contravariant => Some("-"),
            Self::Invariant => Some("="),
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

/// The variance of every polymorphic variable of a struct, enum or `eff`, in
/// argument order.
#[derive(Debug, Clone, PartialEq, Eq, Default, StableHash, Encode, Decode, Identifiable)]
pub struct VarianceMap {
    variances: Vec<Variance>,
    indices_by_id: FxHashMap<PolyVarID, usize>,
}

impl VarianceMap {
    /// Creates a map seeded with declared variances, and bivariant (unused)
    /// elsewhere.
    #[must_use]
    pub fn new(poly_vars: &PolyVarMap) -> Self {
        Self {
            variances: poly_vars
                .iter()
                .map(|(_, poly_var)| poly_var.declared_variance().unwrap_or(Variance::Bivariant))
                .collect(),
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
    #[must_use]
    pub fn get(&self, id: PolyVarID) -> Variance { self.variances[self.index_of(id)] }

    /// Returns the variance of the argument at `index`.
    #[must_use]
    pub fn get_by_index(&self, index: usize) -> Variance { self.variances[index] }

    /// Joins `variance` into the variable's variance for one more use,
    /// returning whether it changed.
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

    /// Makes unused non-lifetime variables invariant so no `PhantomData`
    /// equivalent is needed; returns whether any variance changed.
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

/// Retrieves the declared or inferred variance of every polymorphic variable
/// of a struct, enum or `eff`, in argument order.
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
