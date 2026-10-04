use std::sync::Arc;

use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    SymbolID,
    symbol_kind::{
        AllCallableDefIDs, AllDefWithBodyIDs, AllEffectIDs, AllInstanceIDs, AllNominalTypeIDs,
        AllSymbolIDs, Key, SymbolKind,
    },
};

use crate::index::{get_symbol_table, get_target_tables};

#[executor(config = Config)]
pub async fn symbol_kind_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> SymbolKind {
    engine.get_symbol_table(symbol_id).await.get_symbol_kind(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static SYMBOL_KIND_EXECUTOR: Registration<Config> = Registration::new::<Key, SymbolKindExecutor>();

#[executor(config = Config)]
pub async fn all_symbol_ids_executor(
    &AllSymbolIDs { target }: &AllSymbolIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.all_symbol_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_SYMBOL_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllSymbolIDs, AllSymbolIdsExecutor>();

#[executor(config = Config)]
pub async fn all_def_with_body_ids_executor(
    &AllDefWithBodyIDs { target }: &AllDefWithBodyIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.all_def_with_body_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_DEF_WITH_BODY_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllDefWithBodyIDs, AllDefWithBodyIdsExecutor>();

#[executor(config = Config)]
pub async fn all_instance_ids_executor(
    &AllInstanceIDs { target }: &AllInstanceIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.symbol_ids_of_kind(SymbolKind::Instance)).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_INSTANCE_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllInstanceIDs, AllInstanceIdsExecutor>();

#[executor(config = Config)]
pub async fn all_nominal_type_ids_executor(
    &AllNominalTypeIDs { target }: &AllNominalTypeIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.symbol_ids_of_kind(SymbolKind::Strut)).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_NOMINAL_TYPE_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllNominalTypeIDs, AllNominalTypeIdsExecutor>();

#[executor(config = Config)]
pub async fn all_effect_ids_executor(
    &AllEffectIDs { target }: &AllEffectIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.symbol_ids_of_kind(SymbolKind::Effect)).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_EFFECT_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllEffectIDs, AllEffectIdsExecutor>();

#[executor(config = Config)]
pub async fn all_callable_def_ids_executor(
    &AllCallableDefIDs { target }: &AllCallableDefIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let tables = engine.get_target_tables(target).await;
    tables.iter().flat_map(|table| table.all_callable_def_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_CALLABLE_DEF_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllCallableDefIDs, AllCallableDefIdsExecutor>();
