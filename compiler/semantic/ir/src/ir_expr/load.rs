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
}

impl From<rayc_type::capture::LoadKind> for LoadKind {
    fn from(kind: rayc_type::capture::LoadKind) -> Self {
        match kind {
            rayc_type::capture::LoadKind::Implicit => Self::Implicit,
            rayc_type::capture::LoadKind::Move => Self::Move,
        }
    }
}

/// What a [`Load`] does to the place it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadEffect {
    /// The value is copied, and the place keeps it.
    Copies,

    /// The value is moved out of the place.
    Moves,

    /// The value is copied when its type is `Copy`, and moved otherwise.
    MovesUnlessCopy,
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

    /// Returns what this load does to its place: a forced move always moves,
    /// and an implicit load moves unless the value is `Copy`.
    ///
    /// A load through a dereference never moves: memory behind a raw pointer
    /// is copied bitwise, and moving out of memory behind a reference is an
    /// error reported separately.
    #[must_use]
    pub fn effect(&self) -> LoadEffect {
        if self.address.is_behind_deref() {
            return LoadEffect::Copies;
        }

        match self.kind {
            LoadKind::Implicit => LoadEffect::MovesUnlessCopy,
            LoadKind::Move => LoadEffect::Moves,
        }
    }
}
