use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::span::{DefinitionKey, Key};

use crate::projection::{project, symbol_projection};

symbol_projection!(SpanProjection -> Option<RelativeSpan> = get_span);

#[executor(config = Config)]
pub async fn span_executor(
    &Key { symbol_id }: &Key,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    project(engine, SpanProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static SPAN_EXECUTOR: Registration<Config> = Registration::new::<Key, SpanExecutor>();

symbol_projection!(DefinitionSpanProjection -> Option<RelativeSpan> = get_definition_span);

#[executor(config = Config)]
pub async fn definition_span_executor(
    &DefinitionKey { symbol_id }: &DefinitionKey,
    engine: &TrackedEngine,
) -> Option<RelativeSpan> {
    project(engine, DefinitionSpanProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static DEFINITION_SPAN_EXECUTOR: Registration<Config> =
    Registration::new::<DefinitionKey, DefinitionSpanExecutor>();
