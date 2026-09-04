use qbice::{Decode, Encode, Query, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::trait_ref::TraitRef;

/// Retrieves the trait reference represented by an instance symbol. The value
/// is absent when the trait reference cannot be resolved.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<TraitRef>)]
#[extend(by_val, name = get_instance_trait_ref)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
