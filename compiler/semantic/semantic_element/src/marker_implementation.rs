use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

/// The resolved properties of a marker implementation.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct MarkerImplementation {
    marker_id: Option<GlobalSymbolID>,
    implementor: Interned<Ty>,
    valid_head: bool,
}

impl MarkerImplementation {
    #[must_use]
    pub const fn new(
        marker_id: Option<GlobalSymbolID>,
        implementor: Interned<Ty>,
        valid_head: bool,
    ) -> Self {
        Self { marker_id, implementor, valid_head }
    }

    /// Returns the implemented marker, or `None` when its path did not resolve.
    #[must_use]
    pub const fn marker_id(&self) -> Option<GlobalSymbolID> { self.marker_id }

    /// Returns the type for which the marker is implemented.
    #[must_use]
    pub const fn implementor(&self) -> &Interned<Ty> { &self.implementor }

    /// Returns whether this implementation has a valid simple instance head.
    #[must_use]
    pub const fn has_valid_head(&self) -> bool { self.valid_head }

    /// Returns whether two valid implementations overlap.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.valid_head
            && other.valid_head
            && self.marker_id.is_some()
            && self.marker_id == other.marker_id
            && self.implementor.has_same_type_constructor(&other.implementor)
    }
}

/// Retrieves the resolved properties of a marker-implementation symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<MarkerImplementation>)]
#[extend(by_val, name = get_marker_implementation)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
