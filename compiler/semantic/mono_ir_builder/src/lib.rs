//! Lowers one concrete semantic IR definition, or one generated nominal Drop
//! instance, into an independently cacheable [`rayc_mono_ir::MonoIR`] fragment.
//!
//! This crate intentionally exposes an ordinary lowering function rather than
//! an incremental query. A future orchestrator can use the same
//! `(GlobalSymbolID, Subst)` inputs as its query key.

use rayc_ir::get_ir;
use rayc_mono_ir::{MonoDefInstance, MonoIR, MonoNominalDropInstance};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::context::Context;

mod builder;
mod context;
mod function_abi;
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
    Context::new(solver, instance, source).lower().await
}

/// Lowers the compiler-generated `Drop.drop` body selected by one concrete
/// nominal Drop dictionary into `MonoIR`.
#[must_use]
pub async fn lower_nominal_drop(
    engine: &TrackedEngine,
    instance: MonoNominalDropInstance,
) -> MonoIR {
    nominal_drop::lower(engine, instance).await
}
