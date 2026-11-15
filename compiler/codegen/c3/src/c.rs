//! Rendering of C syntax.
//!
//! Everything in this module is a lightweight adaptor over `MonoIR` data, so
//! generated code is written directly into its output buffer without
//! intermediate allocations. Names and expressions implement
//! [`std::fmt::Display`]. Types implement [`writer::WriteC`] instead, since
//! spelling one asks the engine whether it is zero-sized; the
//! [`writer::CWriter`] is the only way anything here reaches the engine.

pub(crate) mod aggregate;
pub(crate) mod expr;
pub(crate) mod name;
pub(crate) mod ty;
pub(crate) mod writer;
