//! Lowers one concrete semantic IR definition or closure, one generated nominal
//! Drop instance, or the `coroutine_resume` of one coroutine, into an
//! independently cacheable [`rayc_mono_ir::MonoIR`] fragment.
//!
//! This crate intentionally exposes ordinary lowering functions rather than
//! incremental queries. A future orchestrator can use the same
//! `(GlobalSymbolID, Subst)` inputs as its query key.

use rayc_ir::get_ir;
use rayc_mono_ir::{
    MonoClosureInstance, MonoCoroutineInstance, MonoDefInstance, MonoGeneratedAdtDropInstance,
    MonoIR, layout::CoroutineBody,
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::context::Context;

mod context;
mod function_lowering;
mod lower;
mod nominal_drop;
mod resolver;

/// Lowers one concrete source-definition instantiation into `MonoIR`.
#[must_use]
pub async fn lower_ir(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    substitution: Subst,
) -> MonoIR {
    // One solver serves the whole fragment, starting with its own key.
    let source = engine.get_ir(def_id).await;
    let solver = Solver::without_givens(engine.clone()).await;
    let instance = MonoDefInstance::new(def_id, substitution, &solver).await;
    let (context, output) = Context::new_definition(solver, instance, source).await;
    context.lower(output).await
}

/// Lowers one concrete source closure into `MonoIR`.
#[must_use]
pub async fn lower_closure(engine: &TrackedEngine, instance: MonoClosureInstance) -> MonoIR {
    let source = engine.get_ir(instance.owner().def_id()).await;
    let solver = Solver::without_givens(engine.clone()).await;
    let (context, output) = Context::new_closure(solver, instance, source).await;
    context.lower(output).await
}

/// Lowers the compiler-generated `Drop.drop` body selected by one concrete
/// nominal Drop dictionary into `MonoIR`.
#[must_use]
pub async fn lower_nominal_drop(
    engine: &TrackedEngine,
    instance: MonoGeneratedAdtDropInstance,
) -> MonoIR {
    nominal_drop::lower(engine, instance).await
}

/// Lowers the `coroutine_resume` function of one concrete coroutine into
/// `MonoIR`.
#[must_use]
pub async fn lower_coroutine_resume(
    engine: &TrackedEngine,
    instance: MonoCoroutineInstance,
) -> MonoIR {
    let solver = Solver::without_givens(engine.clone()).await;
    let body = CoroutineBody::of(instance.view(), &solver).await;
    let source = engine.get_ir(body.def_id()).await;
    let function_id = body.function_id(&source);
    let (context, output) =
        Context::new_coroutine(solver, body.into_owner(), source, instance, function_id).await;
    context.lower(output).await
}
