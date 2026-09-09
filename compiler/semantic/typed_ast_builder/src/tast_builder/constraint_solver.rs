use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;

use crate::tast_builder::constraint_solver::{
    provenance::{CauseID, Provenance},
    solve::ConstraintSet,
};

mod constraints;
mod diagnostics;
mod provenance;
mod resolution_inference;
mod solve;

// re-exports
pub use provenance::{EffectUnificationSource, SubtypeSource};
pub use resolution_inference::ResolutionInference;
pub use solve::ConstraintError;

#[derive(Debug)]
pub struct ConstraintSolver {
    provenance: Provenance,
    constraint_set: ConstraintSet,
    solver: Solver,
}

impl ConstraintSolver {
    pub async fn new(engine: TrackedEngine, site: GlobalSymbolID) -> Self {
        Self {
            provenance: Provenance::new(),
            constraint_set: ConstraintSet::new(),
            solver: Solver::new_at_site(engine, site).await,
        }
    }
}
