pub mod build;
pub mod diagnostic;
mod effect_row;
mod extern_signature;
pub mod instance_def;
pub mod instance_trait_ref;
mod parameter;
mod poly_var_map;
mod return_type;

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}
