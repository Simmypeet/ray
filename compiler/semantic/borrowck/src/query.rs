//! The queries that borrow check a definition and render what they find.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_ir::{get_ir, ir_function::IRFunctionMap};
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine, unordered::query_all};
use rayc_symbol::{
    GlobalSymbolID, symbol_kind::get_all_def_with_body_ids, syntax::get_def_body_syntax,
};
use rayc_target::TargetID;

use crate::{borrow_check, diagnostic::Diagnostic};

/// Borrow checks the IR of a definition.
///
/// This is a query of its own, and not a step of building the IR, so that a
/// borrow checker goal may ask for the IR of any definition, the checked one
/// included, without a cycle.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Diagnostic]>)]
pub struct BorrowCheck {
    pub def_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn borrow_check_executor(
    &BorrowCheck { def_id }: &BorrowCheck,
    engine: &TrackedEngine,
) -> Interned<[Diagnostic]> {
    let ir = engine.get_ir(def_id).await;

    // Borrow checking runs last, on IR whose drops are elaborated, and only
    // when nothing else went wrong: an earlier error would leave it checking
    // placeholder nodes, and reporting what it finds there would only cascade
    // from that error.
    if !ir.no_error() {
        return engine.intern_unsized([]);
    }

    // The borrow checker renumbers the lifetimes of the IR it checks, so it
    // works on a copy and leaves the queried IR as it was built.
    let mut ir = IRFunctionMap::clone(&ir);
    let diagnostics = borrow_check(&mut ir, engine).await;

    engine.intern_unsized(diagnostics)
}

#[distributed_slice(RAY_PROGRAM)]
static BORROW_CHECK_EXECUTOR: Registration<Config> =
    Registration::new::<BorrowCheck, BorrowCheckExecutor>();

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct SingleRenderedKey {
    pub def_id: GlobalSymbolID,
}

#[executor(config = Config)]
async fn single_rendered_executor(
    &SingleRenderedKey { def_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    if engine.get_def_body_syntax(def_id).await.is_none() {
        return engine.intern_unsized([]);
    }

    let diagnostics = engine.query(&BorrowCheck { def_id }).await;
    let mut rendered = Vec::new();
    for diagnostic in diagnostics.iter() {
        rendered.push(diagnostic.report(engine).await);
    }

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static SINGLE_RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<SingleRenderedKey, SingleRenderedExecutor>();

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Interned<[Rendered<ByteIndex>]>]>)]
pub struct RenderedKey {
    pub target_id: TargetID,
}

#[executor(config = Config)]
async fn rendered_executor(
    &RenderedKey { target_id }: &RenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Interned<[Rendered<ByteIndex>]>]> {
    let def_ids = engine.get_all_def_with_body_ids(target_id).await;

    // the diagnostics of each definition are independent from the others
    let rendered = engine
        .query_all(
            def_ids
                .iter()
                .map(|&def_id| SingleRenderedKey { def_id: target_id.make_global(def_id) }),
        )
        .await;

    engine.intern_unsized(rendered)
}

#[distributed_slice(RAY_PROGRAM)]
static RENDERED_EXECUTOR: Registration<Config> =
    Registration::new::<RenderedKey, RenderedExecutor>();
