use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_symbol::GlobalSymbolID;

use crate::ty::args::Args;

/// A reference to a trait together with its type arguments.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct TraitRef {
    trait_id: GlobalSymbolID,
    args: Args,
}

impl TraitRef {
    #[must_use]
    pub const fn new(trait_id: GlobalSymbolID, args: Args) -> Self { Self { trait_id, args } }

    #[must_use]
    pub const fn trait_id(&self) -> GlobalSymbolID { self.trait_id }

    #[must_use]
    pub const fn args(&self) -> &Args { &self.args }
}
