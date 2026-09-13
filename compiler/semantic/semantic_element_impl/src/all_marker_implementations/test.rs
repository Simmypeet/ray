use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
use rayc_semantic_element::{
    all_marker_implementations::get_all_marker_implementations,
    marker_implementation::{
        Key as MarkerImplementationKey, MarkerImplementation, MarkerImplementationPolarity,
    },
};
use rayc_symbol::{
    SymbolID,
    symbol_kind::{AllSymbolIDs, Key as SymbolKindKey, SymbolKind},
};
use rayc_target::{LinkKey, TargetID};
use rayc_type::ty::{Ty, self_instance::SelfInstance};

use super::AllMarkerImplementationsExecutor;

// input: local -> dependency -> transitive dependency
// premise: valid positive and negative implementations target the same marker
// output: all three implementation IDs in local-to-transitive order
#[tokio::test]
async fn query_recursively_collects_linked_marker_implementations() {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let local = TargetID::new(10);
    let dependency = TargetID::new(20);
    let transitive = TargetID::new(30);
    let marker_id = local.make_global(SymbolID::from_u128(1));
    let local_implementation = local.make_global(SymbolID::from_u128(2));
    let dependency_implementation = dependency.make_global(SymbolID::from_u128(3));
    let transitive_implementation = transitive.make_global(SymbolID::from_u128(4));
    let implementor = engine.intern(Ty::SelfInstance(SelfInstance::new(marker_id)));

    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (AllSymbolIDs { target: local }, Arc::from([local_implementation.id])),
        (AllSymbolIDs { target: dependency }, Arc::from([dependency_implementation.id])),
        (AllSymbolIDs { target: transitive }, Arc::from([transitive_implementation.id])),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (SymbolKindKey { symbol_id: local_implementation }, SymbolKind::MarkerImplementation),
        (SymbolKindKey { symbol_id: dependency_implementation }, SymbolKind::MarkerImplementation),
        (SymbolKindKey { symbol_id: transitive_implementation }, SymbolKind::MarkerImplementation),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (
            MarkerImplementationKey { symbol_id: local_implementation },
            engine.intern(MarkerImplementation::new(
                Some(marker_id),
                implementor.clone(),
                true,
                MarkerImplementationPolarity::Positive,
            )),
        ),
        (
            MarkerImplementationKey { symbol_id: dependency_implementation },
            engine.intern(MarkerImplementation::new(
                Some(marker_id),
                implementor.clone(),
                true,
                MarkerImplementationPolarity::Negative,
            )),
        ),
        (
            MarkerImplementationKey { symbol_id: transitive_implementation },
            engine.intern(MarkerImplementation::new(
                Some(marker_id),
                implementor,
                true,
                MarkerImplementationPolarity::Positive,
            )),
        ),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (LinkKey { target_id: local }, interned_targets(&engine, [dependency])),
        (LinkKey { target_id: dependency }, interned_targets(&engine, [transitive])),
        (LinkKey { target_id: transitive }, interned_targets(&engine, [])),
    ]))));
    engine.register_executor(Arc::new(AllMarkerImplementationsExecutor));

    let engine = Arc::new(engine).tracked().await;
    let implementations = engine.get_all_marker_implementations(marker_id, local).await;

    assert_eq!(&*implementations, &[
        local_implementation,
        dependency_implementation,
        transitive_implementation
    ]);
}

fn interned_targets(
    engine: &Engine,
    targets: impl IntoIterator<Item = TargetID>,
) -> Interned<FxHashSet<TargetID>> {
    engine.intern(targets.into_iter().collect())
}
