//! Diagnostics emitted while building semantic elements.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{get_all_symbol_ids, get_symbol_kind},
};
use rayc_target::TargetID;
use rayc_type::poly_var;

use crate::build::DiagnosticKey;

/// Retrieves all rendered semantic-element diagnostics for a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct SingleRenderedKey {
    /// The target whose semantic-element diagnostics should be rendered.
    symbol_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn single_rendered_executor(
    &SingleRenderedKey { symbol_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    let mut rendered = Vec::new();
    let kind = engine.get_symbol_kind(symbol_id).await;

    if kind.has_effect_row_annotation() {
        let effect_row_key = rayc_semantic_element::effect_row::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(effect_row_key)).await;

        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind.has_parameter_list() {
        let parameter_key = rayc_semantic_element::parameter::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(parameter_key)).await;

        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    if kind.has_return_type() {
        let return_type_key = rayc_semantic_element::return_type::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(return_type_key)).await;

        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    // If the symbol owns polymorphic variables, query for their diagnostics and
    // render them.
    if kind.has_poly_var_map() {
        let key = poly_var::Key { symbol_id };
        let diagnostics = engine.query(&DiagnosticKey::new(key)).await;

        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static SINGLE_RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<SingleRenderedKey, SingleRenderedExecutor>();

/// Retrieves all rendered semantic-element diagnostics for a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[Interned<[Rendered<ByteIndex>]>]>)]
pub struct RenderedKey {
    /// The target whose semantic-element diagnostics should be rendered.
    pub target_id: TargetID,
}

#[executor(config = Config)]
async fn rendered_executor(
    &RenderedKey { target_id }: &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Interned<[Rendered<ByteIndex>]>]> {
    let mut rendered_by_def = Vec::new();
    let ids = engine.get_all_symbol_ids(target_id).await;

    for id in ids.iter().copied().map(|x| target_id.make_global(x)) {
        rendered_by_def.push(engine.query(&SingleRenderedKey { symbol_id: id }).await);
    }

    engine.intern_unsized(rendered_by_def)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();
