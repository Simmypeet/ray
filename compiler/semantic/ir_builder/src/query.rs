use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_borrowck::borrow_check;
use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_ir::ir_function::IRFunctionMap;
use rayc_memory::analyze;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine, unordered::query_all};
use rayc_semantic_element::return_type::get_return_type;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    span::get_span, symbol_kind::get_all_def_with_body_ids, syntax::get_def_body_syntax,
};
use rayc_target::{TargetID, get_ir_verification};
use rayc_typed_ast::get_typed_ast;

use crate::{diagnostic::Diagnostic, erase::erase_lifetimes, lower_function};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value((Interned<IRFunctionMap>, Interned<[Diagnostic]>))]
pub struct BuildIR {
    pub def_id: rayc_symbol::GlobalSymbolID,
}

#[executor(config = Config)]
async fn build_ir_executor(
    &BuildIR { def_id }: &BuildIR,
    engine: &TrackedEngine,
) -> (Interned<IRFunctionMap>, Interned<[Diagnostic]>) {
    let typed_function = engine.get_typed_ast(def_id).await;
    let return_ty = engine.get_return_type(def_id).await;
    let span = if let Some(span) = engine.get_span(def_id).await {
        Some(span)
    } else {
        engine.get_def_body_syntax(def_id).await.map(|body| body.span())
    };
    let (mut function, mut diagnostics) = lower_function(
        engine,
        def_id,
        typed_function.functions(),
        typed_function.captures(),
        return_ty,
        span,
    )
    .await;

    // Resolving a dictionary for a type left erroneous or undetermined by
    // type checking would only repeat that error, so dictionary failures are
    // reported only for a well-typed definition.
    let (_, typed_diagnostics) =
        engine.query(&rayc_typed_ast_builder::query::BuildTAst { def_id }).await;

    if !typed_diagnostics.is_empty() {
        diagnostics.retain(|diagnostic| !matches!(diagnostic, Diagnostic::Memory(_)));
    }

    // Memory checking relies on valid typed and control-flow IR. Keep it out
    // of recovery paths so an earlier error cannot produce misleading move or
    // initialization diagnostics from placeholder nodes. Such IR is never
    // lowered further, so it needs no drops either. A missing dictionary
    // leaves the IR valid, so it does not prevent the check.
    let control_flow_valid = !diagnostics
        .iter()
        .any(|diagnostic| matches!(diagnostic, Diagnostic::NotAllPathsReturnValue(_)));

    if control_flow_valid && typed_diagnostics.is_empty() {
        let memory_diagnostics = analyze(engine, def_id, &mut function).await;
        diagnostics.extend(memory_diagnostics.into_iter().map(Diagnostic::from));

        // Borrow checking runs last, on IR whose drops are elaborated, and
        // only when nothing else went wrong: an earlier error would leave it
        // checking placeholder nodes, and reporting what it finds there would
        // only cascade from that error.
        if diagnostics.is_empty() {
            let borrow_diagnostics = borrow_check(&mut function, engine).await;
            diagnostics.extend(borrow_diagnostics.into_iter().map(Diagnostic::from));
        }
    }

    // Lifetimes are of no use past borrow checking, so they are erased
    // whether or not it ran.
    erase_lifetimes(&mut function, engine).await;

    if engine.get_ir_verification(def_id.target_id).await
        && let Err(error) = crate::verification::verify(&function).await
    {
        panic!("IR verification failed for {def_id:?}: {error}");
    }
    (engine.intern(function), engine.intern_unsized(diagnostics))
}

#[distributed_slice(RAY_PROGRAM)]
static BUILD_IR_EXECUTOR: Registration<Config> = Registration::new::<BuildIR, BuildIrExecutor>();

#[executor(config = Config)]
async fn ir_executor(
    &rayc_ir::Key { def_id }: &rayc_ir::Key,
    engine: &TrackedEngine,
) -> Interned<IRFunctionMap> {
    engine.query(&BuildIR { def_id }).await.0
}

#[distributed_slice(RAY_PROGRAM)]
static IR_EXECUTOR: Registration<Config> = Registration::new::<rayc_ir::Key, IrExecutor>();

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, StableHash, Query,
)]
#[value(Interned<[Rendered<ByteIndex>]>)]
pub struct SingleRenderedKey {
    pub def_id: rayc_symbol::GlobalSymbolID,
}

#[executor(config = Config)]
async fn single_rendered_executor(
    &SingleRenderedKey { def_id }: &SingleRenderedKey,
    engine: &TrackedEngine,
) -> Interned<[Rendered<ByteIndex>]> {
    if engine.get_def_body_syntax(def_id).await.is_none() {
        return engine.intern_unsized([]);
    }

    let (_, diagnostics) = engine.query(&BuildIR { def_id }).await;
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
