use qbice::{Decode, Encode, Identifiable, StableHash};

use crate::{
    poly_var::GlobalPolyVarID,
    ty::{
        application::Application, effect_row::EffectRow, inference::Inference, lifetime::Lifetime,
    },
    variance::Variance,
};

pub mod application;
pub mod args;
mod constructor;
mod display;
pub mod effect_row;
pub mod inference;
mod inspect;
pub mod lifetime;
mod reduce;
pub mod self_instance;
mod subst;
mod view;

pub use display::TyDisplay;
use self_instance::SelfInstance;
pub use view::Dereferenceable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Primitive {
    Integer(Integer),
    Float32,
    Bool,
    CStr,
}

impl Primitive {
    /// Returns the keyword that names this primitive type in source code.
    #[must_use]
    pub const fn keyword(&self) -> &'static str {
        match self {
            Self::Integer(integer) => integer.keyword(),
            Self::Float32 => "float32",
            Self::Bool => "bool",
            Self::CStr => "cstr",
        }
    }
}

/// A primitive integer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Integer {
    Int8,
    Int16,
    Int32,
    Int64,
    Isize,
    Uint8,
    Uint16,
    Uint32,
    Uint64,
    Usize,
    CInt,
}

impl Integer {
    /// Returns the keyword that names this integer type in source code.
    #[must_use]
    pub const fn keyword(&self) -> &'static str {
        match self {
            Self::Int8 => "int8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Int64 => "int64",
            Self::Isize => "isize",
            Self::Uint8 => "uint8",
            Self::Uint16 => "uint16",
            Self::Uint32 => "uint32",
            Self::Uint64 => "uint64",
            Self::Usize => "usize",
            Self::CInt => "c_int",
        }
    }

    /// Returns the largest value of this integer type.
    ///
    /// `isize` and `usize` are bounded by their 64-bit counterparts, and
    /// `c_int` by `int32`, matching how their constants are represented after
    /// monomorphization.
    #[must_use]
    pub const fn max_value(&self) -> u128 {
        match self {
            Self::Int8 => i8::MAX as u128,
            Self::Int16 => i16::MAX as u128,
            Self::Int32 | Self::CInt => i32::MAX as u128,
            Self::Int64 | Self::Isize => i64::MAX as u128,
            Self::Uint8 => u8::MAX as u128,
            Self::Uint16 => u16::MAX as u128,
            Self::Uint32 => u32::MAX as u128,
            Self::Uint64 | Self::Usize => u64::MAX as u128,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Mutability {
    Immutable,
    Mutable,
}

impl Mutability {
    #[must_use]
    pub const fn constness(&self) -> bool {
        match self {
            Self::Immutable => true,
            Self::Mutable => false,
        }
    }

    /// Returns how this mutability is named in diagnostics.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Immutable => "immutable",
            Self::Mutable => "mutable",
        }
    }

    /// Returns the variance of the pointee of a reference with this
    /// mutability: a shared pointee is covariant, and a mutable one is
    /// invariant. Raw pointers are bivariant in their pointee instead.
    #[must_use]
    pub const fn pointee_variance(&self) -> Variance {
        match self {
            Self::Immutable => Variance::Covariant,
            Self::Mutable => Variance::Invariant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum TyKind {
    Star,
    EffectRow,
    Instance,
    Lifetime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum InferenceConstraint {
    Any,
    Numeric,
    EqualityComparable,
}

impl InferenceConstraint {
    #[must_use]
    #[allow(clippy::match_same_arms)]
    pub const fn meet(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Any, Self::Any) => Some(Self::Any),
            (Self::Any, Self::Numeric) | (Self::Numeric, Self::Any) => Some(Self::Numeric),
            (Self::Any, Self::EqualityComparable) | (Self::EqualityComparable, Self::Any) => {
                Some(Self::EqualityComparable)
            }
            (Self::Numeric, Self::Numeric) => Some(Self::Numeric),
            (Self::Numeric, Self::EqualityComparable)
            | (Self::EqualityComparable, Self::Numeric) => Some(Self::Numeric),
            (Self::EqualityComparable, Self::EqualityComparable) => Some(Self::EqualityComparable),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Ty {
    Application(Application),
    Inference(Inference),
    PolyVar(GlobalPolyVarID),
    /// The enclosing trait’s rigid self dictionary; see [`SelfInstance`].
    SelfInstance(SelfInstance),
    EffectRow(EffectRow),
    /// A lifetime that is not a lifetime parameter; see [`Lifetime`].
    Lifetime(Lifetime),
}

#[cfg(test)]
mod test;
