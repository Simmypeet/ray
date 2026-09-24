use qbice::{Decode, Encode, StableHash};

use crate::address::Address;

/// How a [`Load`] treats the value it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum LoadKind {
    /// Copies a `Copy` value and moves any other value.
    Implicit,

    /// Moves the value even when its type is `Copy`, as written with
    /// `move <expr>`.
    Move,

    /// Moves the value even when its type is `Copy`, only to pass it to its
    /// `Drop.drop` call. Inserted by drop elaboration.
    ///
    /// It moves exactly like [`Self::Move`], but analyses which care about
    /// how a value is used, such as liveness, can tell a drop apart from
    /// other uses.
    Drop,
}

impl From<rayc_type::capture::LoadKind> for LoadKind {
    fn from(kind: rayc_type::capture::LoadKind) -> Self {
        match kind {
            rayc_type::capture::LoadKind::Implicit => Self::Implicit,
            rayc_type::capture::LoadKind::Move => Self::Move,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Load {
    address: Address,
    kind: LoadKind,
}

impl Load {
    /// Creates a load which copies a `Copy` value and moves any other value.
    #[must_use]
    pub const fn new(address: Address) -> Self { Self::with_kind(address, LoadKind::Implicit) }

    #[must_use]
    pub const fn with_kind(address: Address, kind: LoadKind) -> Self { Self { address, kind } }

    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }

    #[must_use]
    pub const fn kind(&self) -> LoadKind { self.kind }
}
