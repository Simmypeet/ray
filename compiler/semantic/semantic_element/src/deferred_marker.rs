use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::where_clause::MarkerPredicate;

/// The marker predicate that is deferred until the coroutine's saved-locals are
/// known.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct DeferredMarker {
    marker_predicate: MarkerPredicate,
    /// The span where the marker predicate is incurred, for diagnostics.
    span: RelativeSpan,
    /// The symbol ID that contains the where clause that incurred this marker
    /// predicate, for diagnostics.
    from_symbol_id: GlobalSymbolID,
}

impl DeferredMarker {
    #[must_use]
    pub const fn new(
        marker_predicate: MarkerPredicate,
        span: RelativeSpan,
        from_symbol_id: GlobalSymbolID,
    ) -> Self {
        Self { marker_predicate, span, from_symbol_id }
    }
}
