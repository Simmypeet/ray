use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::span::Key;

use crate::table::get_table;

#[executor(config = Config)]
pub async fn span_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    engine.get_table(symbol_id.target_id).await.get_span(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static SPAN_EXECUTOR: Registration<Config> = Registration::new::<Key, SpanExecutor>();
