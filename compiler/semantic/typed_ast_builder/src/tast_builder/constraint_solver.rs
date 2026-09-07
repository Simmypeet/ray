pub(crate) use provenance::EffectUnificationSource;
pub use provenance::SubtypeSource;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
pub use solve::ConstraintError;

use crate::tast_builder::constraint_solver::{
    provenance::{CauseID, Provenance},
    solve::ConstraintSet,
};

mod constraints;
mod diagnostics;
mod provenance;
mod resolution_inference;
pub(super) use resolution_inference::ResolutionInference;
mod solve;

#[derive(Debug)]
pub struct ConstraintSolver {
    provenance: Provenance,
    constraint_set: ConstraintSet,
    solver: Solver,
}

impl ConstraintSolver {
    #[must_use]
    pub fn new(engine: TrackedEngine, site: GlobalSymbolID) -> Self {
        Self {
            provenance: Provenance::new(),
            constraint_set: ConstraintSet::new(),
            solver: Solver::new_at_site(engine, site),
        }
    }
}
