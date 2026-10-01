use qbice::storage::intern::Interned;
use rayc_ir::ir_function::IRFunctionMap as IrFunctionMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;
use rayc_typed_ast::{capture_plan::CapturePlan, typed_function::TypedFunctionMap};

use crate::{builder::Builder, context::LoweringContext, diagnostic::Diagnostic};

pub mod builder;
mod context;
pub mod diagnostic;
mod erase;
mod expression;
pub mod query;
mod statement;
mod verification;

/// Keeps this crate linked so its distributed query registration is retained.
pub const fn black_box() {}

/// Lowers one typed function into control-flow IR.
///
/// Dictionaries the IR selects itself are resolved in the environment of
/// `def_id`.
pub async fn lower_function(
    engine: &TrackedEngine,
    def_id: GlobalSymbolID,
    functions: &TypedFunctionMap,
    captures: &CapturePlan,
    return_ty: Interned<Ty>,
    span: Option<RelativeSpan>,
) -> (IrFunctionMap, Vec<Diagnostic>) {
    let context = LoweringContext::new(functions, captures);
    Builder::new(engine.clone(), def_id, &context, return_ty, span).await.lower(&context).await
}
