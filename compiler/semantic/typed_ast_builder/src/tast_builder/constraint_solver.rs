use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    ty::{Ty, inference::Inference},
    where_clause::MarkerPredicate,
};
use rayc_typed_ast::capture_plan::CopyOracle;

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
pub use provenance::{EffectUnificationSource, NumericOperation, SubtypeSource};
pub use resolution_inference::ResolutionInference;
pub use solve::ConstraintError;

#[derive(Debug)]
pub struct ConstraintSolver {
    provenance: Provenance,
    constraint_set: ConstraintSet,
    solver: Solver,
}

/// Answers `Copy` queries under the substitution solved so far.
///
/// An undetermined numeric type is `Copy`, since it defaults to a numeric
/// primitive. Any other type which is not determined yet counts as not `Copy`.
impl CopyOracle for ConstraintSolver {
    async fn is_copy(&mut self, ty: &Interned<Ty>) -> bool {
        let ty = self.latest_type(ty).await;

        // any numeric type is `Copy`, even if it is undetermined.
        if let Ty::Inference(inference) = &*ty
            && inference.constraint().default_primitive().is_some()
        {
            return true;
        }

        let copy = self.solver.engine().get_core_item(CoreItem::Copy).await;
        self.solver.entails_marker_predicate(MarkerPredicate::new(copy, ty)).await
    }
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

    /// Takes every lifetime inference generated for this definition so far,
    /// all of which the solver created while generalizing.
    fn take_recorded_lifetime_inferences(&mut self) -> Vec<Inference> {
        self.recorded_mut().take_lifetimes()
    }

    /// Takes every numeric inference generated for this definition so far.
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
