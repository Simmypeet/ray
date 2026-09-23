use std::any::Any;

use rayc_solver::inference_generator::{CountingInferenceGenerator, InferenceGenerator};
use rayc_type::ty::{InferenceConstraint, TyKind, inference::Inference};

/// Records the inferences generated while typing one definition that are
/// defaulted once the constraints are solved: numeric literals and effect
/// rows. Every inference passes through here, including those the solver
/// creates internally.
#[derive(Debug, Default)]
pub(super) struct RecordingInferenceGenerator {
    counter: CountingInferenceGenerator,
    numerics: Vec<Inference>,
    effect_rows: Vec<Inference>,
}

impl RecordingInferenceGenerator {
    /// Moves out the numeric inferences recorded so far. Later ones are
    /// recorded afresh.
    pub(super) fn take_numerics(&mut self) -> Vec<Inference> { std::mem::take(&mut self.numerics) }

    /// Moves out the effect rows recorded so far. Later ones are recorded
    /// afresh.
    pub(super) fn take_effect_rows(&mut self) -> Vec<Inference> {
        std::mem::take(&mut self.effect_rows)
    }
}

impl InferenceGenerator for RecordingInferenceGenerator {
    fn generate(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        let inference = self.counter.generate(kind, constraint);
        if kind == TyKind::Star && constraint == InferenceConstraint::Numeric {
            self.numerics.push(inference);
        }
        if kind == TyKind::EffectRow {
            self.effect_rows.push(inference);
        }
        inference
    }

    fn as_any(&self) -> &dyn Any { self }

    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}
