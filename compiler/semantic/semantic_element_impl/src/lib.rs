mod all_instance_implements_trait;
mod associated_type_kind;
pub mod build;
pub mod diagnostic;
mod effect_row;
mod extern_signature;
pub mod instance_member;
pub mod instance_trait_ref;
mod obligation;
mod parameter;
mod poly_var_map;
mod return_type;

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}

mod type_definition;
mod where_clause;
