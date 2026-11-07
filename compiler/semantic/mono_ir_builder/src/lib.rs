//! Lowers one concrete function into an independently cacheable
//! [`MonoFunction`]: one function of the semantic IR of a concrete definition,
//! one generated nominal Drop instance, or the `coroutine_resume` of one
//! coroutine.
//!
//! This crate intentionally exposes an ordinary lowering function rather than
//! an incremental query. A future orchestrator can use the same
//! [`MonoFunctionInstance`] input as its query key.

use rayc_ir::get_ir;
use rayc_mono_ir::{
    MonoBodyInstance, MonoCoroutineInstance, MonoFunctionInstance, function::MonoFunction,
    signature::get_coroutine_signature,
};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;

use crate::{context::Context, emitter::Emitter, resolver::Resolver};

mod context;
mod emitter;
mod lower;
mod nominal_drop;
mod resolver;

/// Lowers the function that `instance` identifies into `MonoFunction`.
///
/// # Panics
///
/// Panics for an `extern def`, which has no body to lower: it is only
/// declared.
#[must_use]
pub async fn lower_function(
    engine: &TrackedEngine,
    instance: MonoFunctionInstance,
) -> MonoFunction {
    match instance {
        MonoFunctionInstance::Body(instance) => lower_body(engine, instance).await,
        MonoFunctionInstance::ExternDef(_) => panic!("an `extern def` has no body to lower"),
        MonoFunctionInstance::NominalDrop(instance) => nominal_drop::lower(engine, instance).await,
        MonoFunctionInstance::CoroutineResume(instance) => {
            lower_coroutine_resume(engine, instance).await
        }
    }
}

/// Lowers one function of the IR of a concrete definition in callback style.
async fn lower_body(engine: &TrackedEngine, instance: MonoBodyInstance) -> MonoFunction {
    // One solver serves the whole function, in the instantiation of its
    // definition.
    let solver = Solver::without_givens(engine.clone()).await;
    let resolver = Resolver::new(solver, instance.substitution().clone());
    let source = engine.get_ir(instance.def_id()).await;

    // The function implements the signature its callers spell.
    let signature = resolver.callback_signature(&instance).await;
    let mut output = MonoFunction::new(instance.clone(), signature, engine);

    let mut emit = Emitter::new(&resolver, &mut output);
    let ctx = Context::new_callback(&resolver, instance, source, &mut emit).await;
    Box::pin(emit.lower_function(&ctx)).await;

    output
}

/// Lowers the `coroutine_resume` function of one concrete coroutine.
async fn lower_coroutine_resume(
    engine: &TrackedEngine,
    instance: MonoCoroutineInstance,
) -> MonoFunction {
    // One solver serves the whole function, in the instantiation of the
    // definition of the suspended function.
    let solver = Solver::without_givens(engine.clone()).await;
    let resolver = Resolver::new(solver, instance.body().substitution().clone());
    let source = engine.get_ir(instance.body().def_id()).await;

    let signature = engine.get_coroutine_signature(instance.clone()).await;
    let mut output =
        MonoFunction::new(instance.clone(), signature.resume_signature().clone(), engine);

    let mut emit = Emitter::new(&resolver, &mut output);
    let ctx = Context::new_coroutine(&resolver, &instance, source, &mut emit).await;
    Box::pin(emit.lower_coroutine(&ctx)).await;

    output
}
