use qbice::{Decode, Encode, Identifiable, StableHash};

use crate::ty::Mutability;

/// How a load reads a value out of a place.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum LoadKind {
    /// Copies a `Copy` value and moves any other value.
    Implicit,

    /// Moves the value even when its type is `Copy`, as written with
    /// `move <expr>`.
    Move,
}

impl LoadKind {
    /// A forced move by any use consumes the value.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Implicit, Self::Implicit) => Self::Implicit,
            (Self::Move, Self::Implicit | Self::Move) | (Self::Implicit, Self::Move) => Self::Move,
        }
    }
}

/// How a closure receives an outer binding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum CaptureMode {
    /// The binding is loaded into the environment with the given kind.
    Value(LoadKind),
    Reference(Mutability),
}

impl CaptureMode {
    /// Combines the requirements of two uses, choosing the strongest.
    ///
    /// A value requirement moves the binding into the closure, and the
    /// closure's own copy then also serves every by-reference use. Otherwise
    /// a mutable reference is needed if either use mutates the binding.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Value(left), Self::Value(right)) => Self::Value(left.join(right)),
            (mode @ Self::Value(_), Self::Reference(_))
            | (Self::Reference(_), mode @ Self::Value(_)) => mode,
            (Self::Reference(Mutability::Immutable), Self::Reference(Mutability::Immutable)) => {
                Self::Reference(Mutability::Immutable)
            }
            (
                Self::Reference(Mutability::Mutable),
                Self::Reference(Mutability::Immutable | Mutability::Mutable),
            )
            | (Self::Reference(Mutability::Immutable), Self::Reference(Mutability::Mutable)) => {
                Self::Reference(Mutability::Mutable)
            }
        }
    }
}
