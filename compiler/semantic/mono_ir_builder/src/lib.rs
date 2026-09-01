//! Lowers one concrete semantic IR definition into an independently cacheable
//! [`rayc_mono_ir::MonoIR`] fragment.
//!
//! This crate intentionally exposes an ordinary lowering function rather than
//! an incremental query. A future orchestrator can use the same
//! `(GlobalSymbolID, Subst)` inputs as its query key.

use rayc_ir::get_ir;
use rayc_mono_ir::{MonoDefInstance, MonoIR};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::builder::Builder;

mod builder;
mod function_abi;

/// Lowers one concrete source-definition instantiation into `MonoIR`.
#[must_use]
pub async fn lower_ir(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    substitution: Subst,
) -> MonoIR {
    let source = engine.get_ir(def_id).await;
    Builder::new(engine.clone(), MonoDefInstance::new(def_id, substitution), source).lower().await
}
