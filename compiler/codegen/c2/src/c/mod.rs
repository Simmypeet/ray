//! Pure, synchronous rendering of C syntax.
//!
//! Everything in this module is a lightweight [`std::fmt::Display`] adaptor
//! over `MonoIR` data, so generated code is written directly into its output
//! buffer without intermediate allocations. Nothing here queries the compiler
//! engine; facts that require queries are resolved beforehand.

pub(crate) mod aggregate;
pub(crate) mod expr;
pub(crate) mod name;
pub(crate) mod ty;
