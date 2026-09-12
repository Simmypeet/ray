use qbice::{Decode, Encode, Identifiable, StableHash};

use crate::ty::Mutability;

/// How a closure receives an outer binding.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum CaptureMode {
    Value,
    Reference(Mutability),
}

impl CaptureMode {
    /// A reference requirement preserves access to the original binding.
    #[must_use]
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Value, mode) | (mode, Self::Value) => mode,
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
