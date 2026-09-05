use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

/// Retrieves implicitly eligible instances in a target for the given trait.
///
/// Every declaration-owned ordinary type/effect parameter must occur in the
/// normalized trait reference. Given parameters are exempt; their requirements
/// are resolved during instance search, not by this query. Unresolved heads and
/// heads containing errors or inference variables are omitted without
/// additional diagnostics. Ineligible instances remain explicitly usable.
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
