use qbice::{Decode, Encode, StableHash};

use crate::address::Address;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct RefOf {
    address: Address,
}

impl RefOf {
    #[must_use]
    pub const fn new(address: Address) -> Self { Self { address } }
}
