//! The variance of type constructor parameters.
//!
//! Variance is the four-point lattice with [`Variance::Bivariant`] at the
//! bottom, [`Variance::Invariant`] at the top, and the covariant and
//! contravariant points in between. The variances of built-in constructors are
//! fixed. Those of struct and `eff` parameters are computed from their
//! declarations; see [`VarianceKey`] and [`EffectVarianceKey`].

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

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

/// Retrieves the variance of every polymorphic variable of a struct, in the
/// order of its poly var map, which is also the order of its arguments.
///
/// An unused lifetime parameter is [`Variance::Bivariant`] and is reported as
/// an error. Every other unused parameter is [`Variance::Invariant`], so no
/// `PhantomData` equivalent is needed.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Variance]>)]
#[extend(by_val, name = get_variance)]
pub struct VarianceKey {
    /// A `SymbolKind::Strut` symbol.
    pub struct_id: GlobalSymbolID,
}

/// Retrieves the variance of every polymorphic variable of an `eff`, in the
/// order of its poly var map, which is also the order of a label's arguments.
///
/// A `perform` calls the handler, so a parameter used in an operation's
/// parameter types is covariant, one used in its return type is
/// contravariant, and one used in both is invariant. Unused parameters follow
/// the same rule as [`VarianceKey`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Variance]>)]
#[extend(by_val, name = get_effect_variance)]
pub struct EffectVarianceKey {
    /// A `SymbolKind::Effect` symbol.
    pub effect_id: GlobalSymbolID,
}
