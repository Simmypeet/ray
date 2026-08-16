use linkme::distributed_slice;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::scope_span::Key;
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
#[value(Option<Option<RelativeSpan>>)]
pub struct ProjectionKey {
    pub symbol_id: Global<rayc_symbol::SymbolID>,
}

#[executor(config = Config, style = qbice::ExecutionStyle::Projection)]
async fn projection_executor(
    key: &ProjectionKey,
    engine: &TrackedEngine,
) -> Option<Option<RelativeSpan>> {
    let id = key.symbol_id;
    let table = engine.get_table_of_symbol(id).await?;

    table.scope_spans.get(&id.id).copied()
}

#[distributed_slice(RAY_PROGRAM)]
static PROJECTION_EXECUTOR: Registration<Config> =
    Registration::new::<ProjectionKey, ProjectionExecutor>();

#[executor(config = Config)]
async fn scope_span_executor(
    key: &Key,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    engine.query(&ProjectionKey { symbol_id: key.symbol_id }).await.unwrap()
}

#[distributed_slice(RAY_PROGRAM)]
static SCOPE_SPAN_EXECUTOR: Registration<Config> =
    Registration::new::<Key, ScopeSpanExecutor>();
