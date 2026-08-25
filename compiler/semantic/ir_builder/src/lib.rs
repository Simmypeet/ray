use rayc_ir::function::FunctionMap as IrFunctionMap;
use rayc_qbice::TrackedEngine;
use rayc_tast_capture_analysis::CaptureAnalysis;
use rayc_typed_ast::typed_function::TypedFunctionMap;

use crate::{builder::Builder, context::LoweringContext};

mod address;
mod builder;
mod context;
mod expression;
mod function_build_state;
mod query;
mod statement;

#[cfg(test)]
mod tests;

/// Keeps this crate linked so its distributed query registration is retained.
pub const fn black_box() {}

/// Lowers one typed function into control-flow IR.
#[must_use]
pub fn lower_function(engine: &TrackedEngine, functions: &TypedFunctionMap) -> IrFunctionMap {
    let analysis = CaptureAnalysis::analyze(functions);
    let context = LoweringContext::new(functions, &analysis);
    Builder::new(engine.clone(), &context).lower(&context)
}
