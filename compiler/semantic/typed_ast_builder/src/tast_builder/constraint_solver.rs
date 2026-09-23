use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::inference::Inference;

use crate::tast_builder::constraint_solver::{
    inference_generator::RecordingInferenceGenerator,
    provenance::{CauseID, Provenance},
    solve::ConstraintSet,
};

mod constraints;
mod diagnostics;
mod inference_generator;
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
            solver: Solver::new(engine, site)
                .await
                .with_inference_generator(Box::new(RecordingInferenceGenerator::default())),
        }
    }

    /// Takes every numeric literal inference generated for this definition so
    /// far.
    fn take_recorded_numeric_inferences(&mut self) -> Vec<Inference> {
        self.recorded_mut().take_numerics()
    }

    /// Takes every effect row generated for this definition so far, including
    /// those the solver created internally.
    fn take_recorded_effect_row_inferences(&mut self) -> Vec<Inference> {
        self.recorded_mut().take_effect_rows()
    }

    fn recorded_mut(&mut self) -> &mut RecordingInferenceGenerator {
        self.solver
            .inference_generator_mut()
            .as_any_mut()
            .downcast_mut::<RecordingInferenceGenerator>()
            .expect("the constraint solver installs a recording inference generator")
    }
}
