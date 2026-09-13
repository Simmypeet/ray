use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_hash::FxHashSet;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    all_marker_implementations::{AllMarkerImplementations, get_all_marker_implementations},
    marker_implementation::get_marker_implementation,
};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_all_symbol_ids, get_symbol_kind},
};
use rayc_target::get_linked_targets;

#[executor(config = Config)]
async fn all_marker_implementations_executor(
    &AllMarkerImplementations { marker_id, target_id }: &AllMarkerImplementations,
    engine: &TrackedEngine,
) -> Interned<[GlobalSymbolID]> {
    let symbol_ids = engine.get_all_symbol_ids(target_id).await;
    let mut implementations = Vec::new();
    let mut seen = FxHashSet::default();

    // Resolve marker declarations of either polarity and retain only valid
    // implementations of the requested marker.
    for id in symbol_ids.iter().copied() {
        let symbol_id = target_id.make_global(id);
        if engine.get_symbol_kind(symbol_id).await != SymbolKind::MarkerImplementation {
            continue;
        }

        let implementation = engine.get_marker_implementation(symbol_id).await;
        if implementation.has_valid_head()
            && implementation.marker_id() == Some(marker_id)
            && seen.insert(symbol_id)
        {
            implementations.push(symbol_id);
        }
    }

    // Query linked targets recursively so each dependency remains an explicit
    // incremental query dependency.
    for linked_target in engine.get_linked_targets(target_id).await.iter().copied() {
        let linked = engine.get_all_marker_implementations(marker_id, linked_target).await;
        for &implementation in linked.iter() {
            if seen.insert(implementation) {
                implementations.push(implementation);
            }
        }
    }

    engine.intern_unsized(implementations)
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_MARKER_IMPLEMENTATIONS_EXECUTOR: Registration<Config> =
    Registration::new::<AllMarkerImplementations, AllMarkerImplementationsExecutor>();

#[cfg(test)]
mod test;
