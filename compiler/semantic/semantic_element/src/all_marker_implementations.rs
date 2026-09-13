use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_target::TargetID;

/// Retrieves the valid marker implementations visible from a target for the
/// given marker.
///
/// Both positive and negative implementations are returned. Implementations
/// whose marker path did not resolve or whose head is invalid are omitted.
/// The specified target is searched before recursively searching its linked
/// targets.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[GlobalSymbolID]>)]
#[extend(by_val, name = get_all_marker_implementations)]
pub struct AllMarkerImplementations {
    /// The marker implemented by the declarations to retrieve.
    pub marker_id: GlobalSymbolID,
    /// The target containing the implementations to search.
    pub target_id: TargetID,
}
