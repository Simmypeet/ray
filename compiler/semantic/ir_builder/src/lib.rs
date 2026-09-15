use qbice::storage::intern::Interned;
use rayc_ir::ir_function::IRFunctionMap as IrFunctionMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::ty::Ty;
use rayc_typed_ast::{capture_plan::CapturePlan, typed_function::TypedFunctionMap};

use crate::{builder::Builder, context::LoweringContext, diagnostic::NotAllPathsReturnValue};

mod address;
mod builder;
mod context;
pub mod diagnostic;
mod expression;
pub mod query;
mod statement;

/// Keeps this crate linked so its distributed query registration is retained.
pub const fn black_box() {}

/// Lowers one typed function into control-flow IR.
#[must_use]
pub fn lower_function(
    engine: &TrackedEngine,
    functions: &TypedFunctionMap,
    captures: &CapturePlan,
    return_ty: Interned<Ty>,
    span: Option<RelativeSpan>,
) -> (IrFunctionMap, Vec<NotAllPathsReturnValue>) {
    let context = LoweringContext::new(functions, captures);
    Builder::new(engine.clone(), &context, return_ty, span).lower(&context)
}
