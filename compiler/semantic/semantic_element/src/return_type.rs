//! The return type of a function symbol. The query is declared in
//! [`rayc_type::signature`], where type reduction can reach it.

// REVIEW: Let's remove this re-export and refactor the usages to use the
// original definitions

pub use rayc_type::signature::{ReturnTypeKey as Key, get_return_type};
