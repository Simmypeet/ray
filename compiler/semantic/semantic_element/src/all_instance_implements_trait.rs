use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

/// Retrieves all instances in a target whose resolved trait reference names the
/// given trait. Instances with unresolved trait references are omitted.
///
/// Only the specified target is scanned; its dependencies are not searched.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[GlobalSymbolID]>)]
#[extend(by_val, name = get_all_instance_implements_trait)]
pub struct AllInstanceImplementsTrait {
    /// The trait implemented by the instances to retrieve.
    pub trait_id: GlobalSymbolID,
    /// The target containing the instances to search.
    pub target_id: TargetID,
}
