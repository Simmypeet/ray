use rayc_ir::function::Function as IrFunction;
use rayc_qbice::TrackedEngine;
use rayc_typed_ast::function::Function as TypedFunction;

use crate::builder::Builder;

mod address;
mod builder;
mod expression;
mod query;
mod statement;

/// Keeps this crate linked so its distributed query registration is retained.
pub const fn black_box() {}

/// Lowers one typed function into control-flow IR.
#[must_use]
pub fn lower_function(engine: &TrackedEngine, function: &TypedFunction) -> IrFunction {
    Builder::new(engine.clone()).lower(function)
}
