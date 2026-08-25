//! Diagnostics emitted while building semantic elements.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::symbol_kind::get_all_callable_def_ids;
use rayc_target::TargetID;

use crate::{build::DiagnosticKey, function_signature};

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
    let def_ids = engine.get_all_callable_def_ids(target_id).await;

    for def_id in def_ids.iter().copied() {
        let key = function_signature::Key { symbol_id: target_id.make_global(def_id) };
        let diagnostics = engine.query(&DiagnosticKey::new(key)).await;
        let mut rendered = Vec::new();

        for diagnostic in diagnostics.iter() {
            rendered.push(diagnostic.report(engine).await);
        }

        rendered_by_def.push(engine.intern_unsized(rendered));
    }

    engine.intern_unsized(rendered_by_def)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();
