use qbice::{Decode, Encode, StableHash};

use crate::address::Address;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Load {
    address: Address,
}

impl Load {
    #[must_use]
    pub const fn new(address: Address) -> Self { Self { address } }

    #[must_use]
    pub const fn address(&self) -> &Address { &self.address }
}
