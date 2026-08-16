use linkme::distributed_slice;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::linkage::{Key, Linkage};
use rayc_target::Global;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration,
};

use crate::table::get_table_of_symbol;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    Query,
)]
#[value(Option<Linkage>)]
pub struct ProjectionKey {
    pub symbol_id: Global<rayc_symbol::SymbolID>,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn projection_executor(
    key: &ProjectionKey,
    engine: &TrackedEngine,
) -> Option<Linkage> {
    let id = key.symbol_id;
    let table = engine.get_table_of_symbol(id).await?;

    table.function_linkages.get(&id.id).copied()
}

#[distributed_slice(RAY_PROGRAM)]
static PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<ProjectionKey, ProjectionExecutor>();

#[executor(config = Config)]
async fn linkage_executor(key: &Key, engine: &TrackedEngine) -> Linkage {
    engine.query(&ProjectionKey { symbol_id: key.symbol_id }).await.unwrap()
}

#[distributed_slice(RAY_PROGRAM)]
static LINKAGE_EXECUTOR: Registration<Config> =
    Registration::new::<Key, LinkageExecutor>();
