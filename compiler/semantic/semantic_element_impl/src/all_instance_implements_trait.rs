use linkme::distributed_slice;
use qbice::{executor, program::Registration, storage::intern::Interned};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    all_instance_implements_trait::AllInstanceImplementsTrait,
    instance_trait_ref::get_instance_trait_ref,
};
use rayc_symbol::{GlobalSymbolID, symbol_kind::get_all_instance_ids};

#[executor(config = Config)]
async fn all_instance_implements_trait_executor(
    &AllInstanceImplementsTrait { trait_id, target_id }: &AllInstanceImplementsTrait,
    engine: &TrackedEngine,
) -> Interned<[GlobalSymbolID]> {
    let instance_ids = engine.get_all_instance_ids(target_id).await;
    let mut instances = Vec::new();

    for &id in instance_ids.iter() {
        let symbol_id = target_id.make_global(id);
        if engine
            .get_instance_trait_ref(symbol_id)
            .await
            .is_some_and(|trait_ref| trait_ref.trait_id() == trait_id)
        {
            instances.push(symbol_id);
        }
    }

    engine.intern_unsized(instances)
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_INSTANCE_IMPLEMENTS_TRAIT_EXECUTOR: Registration<Config> =
    Registration::new::<AllInstanceImplementsTrait, AllInstanceImplementsTraitExecutor>();
