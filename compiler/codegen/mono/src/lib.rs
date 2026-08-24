//! Backend-independent collection of concrete code-generation instantiations.
//!
//! Monomorphization runs only after semantic analysis has completed
//! successfully. Consequently, every inspected type must be concrete and
//! error-free. Violations indicate a compiler pipeline bug rather than a
//! recoverable source-program error, so the collector reports them by panicking
//! at the point where the invalid type is observed.

mod collect;
mod model;

pub use collect::collect_target;
pub use model::{MonoFunction, MonoProgram, MonoTuple};
