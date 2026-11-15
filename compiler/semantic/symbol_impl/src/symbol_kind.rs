//! The kind of each symbol, a projection of its table, and the symbols of a
//! target by kind.
//!
//! The symbols of a target by kind read the summary of every table of the
//! target, which changes only when a symbol is added or removed. They are
//! firewalls, so that their dependents, such as the instance resolution of
//! every function, recompute only when a symbol of their kind is.

use std::sync::Arc;

use linkme::distributed_slice;
use qbice::{executor, program::Registration};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    SymbolID,
    symbol_kind::{
        AllAdtIDs, AllCallableDefIDs, AllDefWithBodyIDs, AllEffectIDs, AllInstanceIDs,
        AllMarkerImplementationIDs, AllSymbolIDs, Key, SymbolKind,
    },
};

use crate::{
    index::get_target_table_summaries,
    projection::{project, symbol_projection},
};

symbol_projection!(SymbolKindProjection -> SymbolKind = get_symbol_kind);

#[executor(config = Config)]
pub async fn symbol_kind_executor(&Key { symbol_id }: &Key, engine: &TrackedEngine) -> SymbolKind {
    project(engine, SymbolKindProjection { symbol_id }).await
}

#[distributed_slice(RAY_PROGRAM)]
static SYMBOL_KIND_EXECUTOR: Registration<Config> = Registration::new::<Key, SymbolKindExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_symbol_ids_executor(
    &AllSymbolIDs { target }: &AllSymbolIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.all_symbol_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_SYMBOL_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllSymbolIDs, AllSymbolIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_def_with_body_ids_executor(
    &AllDefWithBodyIDs { target }: &AllDefWithBodyIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.all_def_with_body_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_DEF_WITH_BODY_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllDefWithBodyIDs, AllDefWithBodyIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_instance_ids_executor(
    &AllInstanceIDs { target }: &AllInstanceIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.symbol_ids_of_kind(SymbolKind::Instance)).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_INSTANCE_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllInstanceIDs, AllInstanceIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_marker_implementation_ids_executor(
    &AllMarkerImplementationIDs { target }: &AllMarkerImplementationIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries
        .iter()
        .flat_map(|summary| summary.symbol_ids_of_kind(SymbolKind::MarkerImplementation))
        .collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_MARKER_IMPLEMENTATION_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllMarkerImplementationIDs, AllMarkerImplementationIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_adt_ids_executor(
    &AllAdtIDs { target }: &AllAdtIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.all_adt_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_ADT_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllAdtIDs, AllAdtIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_effect_ids_executor(
    &AllEffectIDs { target }: &AllEffectIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.symbol_ids_of_kind(SymbolKind::Effect)).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_EFFECT_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllEffectIDs, AllEffectIdsExecutor>();

#[executor(config = Config, style = qbice::ExecutionStyle::Firewall)]
pub async fn all_callable_def_ids_executor(
    &AllCallableDefIDs { target }: &AllCallableDefIDs,
    engine: &TrackedEngine,
) -> Arc<[SymbolID]> {
    let summaries = engine.get_target_table_summaries(target).await;
    summaries.iter().flat_map(|summary| summary.all_callable_def_ids()).collect()
}

#[distributed_slice(RAY_PROGRAM)]
static ALL_CALLABLE_DEF_IDS_EXECUTOR: Registration<Config> =
    Registration::new::<AllCallableDefIDs, AllCallableDefIdsExecutor>();
