use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::span::{DefinitionKey, Key};

use crate::index::get_symbol_table;

#[executor(config = Config)]
pub async fn span_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    engine.get_symbol_table(symbol_id).await.get_span(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static SPAN_EXECUTOR: Registration<Config> = Registration::new::<Key, SpanExecutor>();

#[executor(config = Config)]
pub async fn definition_span_executor(
    &DefinitionKey { symbol_id }: &DefinitionKey,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    engine.get_symbol_table(symbol_id).await.get_definition_span(symbol_id.id)
}

#[distributed_slice(RAY_PROGRAM)]
static DEFINITION_SPAN_EXECUTOR: Registration<Config> =
    Registration::new::<DefinitionKey, DefinitionSpanExecutor>();
