pub mod context;
pub mod expression;
pub mod function;
pub mod function_ctx;
#[allow(dead_code, reason = "the parallel IR path is wired to production in migration stage 4")]
mod ir;
pub mod statement;
mod translation_unit;
pub mod ty;
pub mod write;
pub mod writer;

pub use translation_unit::write_c_translation_unit;
