use qbice::{Decode, Encode, StableHash};

use crate::name_binding::NameBindingID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Identifier {
    name_binding: NameBindingID,
}

impl Identifier {
    #[must_use]
    pub const fn new(name_binding: NameBindingID) -> Self { Self { name_binding } }

    #[must_use]
    pub const fn name_binding(&self) -> NameBindingID { self.name_binding }
}
