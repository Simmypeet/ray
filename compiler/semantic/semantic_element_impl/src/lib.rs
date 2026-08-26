pub mod build;
pub mod diagnostic;
mod extern_signature;
mod parameter;
mod poly_var_map;
mod return_type;

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}
