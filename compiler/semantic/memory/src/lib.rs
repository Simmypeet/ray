//! State tracked by stack-memory dataflow analyses.

mod analysis;
pub mod diagnostic;
mod drop_elaboration;
pub mod drop_resolution;
mod place_state;
mod problem;
mod stack_state;

pub use analysis::analyze;
pub use diagnostic::Diagnostic;
pub use place_state::{MoveHistory, PlaceState, PossibleStates};
pub use problem::StackStateProblem;
pub use stack_state::{StackRoot, StackSlots, StackState};

#[cfg(test)]
mod tests;
