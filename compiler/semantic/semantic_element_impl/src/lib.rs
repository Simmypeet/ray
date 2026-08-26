pub mod build;
pub mod diagnostic;
mod function_signature;
mod poly_var_map;

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}
