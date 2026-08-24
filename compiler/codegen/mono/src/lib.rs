mod collect;
mod error;
mod model;

pub use collect::collect_target;
pub use error::MonoError;
pub use model::{MonoFunction, MonoProgram, MonoTuple};
