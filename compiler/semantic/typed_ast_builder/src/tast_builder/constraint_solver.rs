pub use provenance::SubtypeSource;
use rayc_qbice::TrackedEngine;
use rayc_type::solver::Solver;

use crate::tast_builder::constraint_solver::{
    provenance::{CauseID, Provenance},
    solve::ConstraintSet,
};

mod constraints;
mod diagnostics;
mod provenance;
mod solve;

#[derive(Debug)]
pub struct ConstraintSolver {
    provenance: Provenance,
    constraint_set: ConstraintSet,
    solver: Solver,
}

impl ConstraintSolver {
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self {
        Self {
            provenance: Provenance::new(),
            constraint_set: ConstraintSet::new(),
            solver: Solver::new(engine),
        }
    }
}
