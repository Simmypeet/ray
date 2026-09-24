use qbice::{Decode, Encode, StableHash};
use rayc_type::capture::LoadKind;

use crate::address::Address;

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
