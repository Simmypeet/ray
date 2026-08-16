use std::sync::Arc;

use linkme::distributed_slice;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    SymbolID,
    symbol_kind::{AllDefIDs, AllSymbolIDs, Key, SymbolKind},
};
use qbice::{executor, program::Registration};

use crate::table::get_table;

#[executor(config = Config)]
pub async fn symbol_kind_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> SymbolKind {
    engine.get_table(symbol_id.target_id).await.get_symbol_kind(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static SYMBOL_KIND_EXECUTOR: Registration<Config> = Registration::new::<Key, SymbolKindExecutor>();

#[executor(config = Config)]
pub async fn all_symbol_ids_executor(
    &AllSymbolIDs { target }: &AllSymbolIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let table = engine.get_table(target).await;
    table.all_symbol_ids().collect::<Arc<_>>()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_SYMBOL_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllSymbolIDs, AllSymbolIdsExecutor>();

#[executor(config = Config)]
pub async fn all_def_ids_executor(
    &AllDefIDs { target }: &AllDefIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let table = engine.get_table(target).await;
    table.all_def_ids().collect::<Arc<_>>()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_DEF_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllDefIDs, AllDefIdsExecutor>();
