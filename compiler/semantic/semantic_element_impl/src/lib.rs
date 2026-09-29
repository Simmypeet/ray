mod all_instance_implements_trait;
mod all_marker_implementations;
mod associated_type_kind;
pub mod build;
pub mod diagnostic;
mod effect_row;
mod extern_signature;
mod inferred_outlives;
pub mod instance_member;
pub mod instance_trait_ref;
mod marker_implementation;
mod obligation;
mod parameter;
mod poly_var_map;
mod return_type;
mod struct_body;

/// A dummy function to make sure this crate is linked by the compiler.
pub const fn black_box() {}

mod callable_parameter;
mod drop_plan;
mod type_definition;
mod variance;
mod where_clause;
