use rayc_qbice::TrackedEngine;

use crate::ty::{InferenceConstraint, TyKind, inference::Inference};

#[derive(Debug, Clone)]
pub struct Solver {
    inference_counter: u64,
    engine: TrackedEngine,
}

impl Solver {
    #[must_use]
    pub const fn new(engine: TrackedEngine) -> Self { Self { inference_counter: 0, engine } }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    #[must_use]
    pub const fn new_inference(&mut self, kind: TyKind) -> Inference {
        self.new_inference_with_constraint(kind, InferenceConstraint::Any)
    }

    #[must_use]
    pub const fn new_inference_with_constraint(
        &mut self,
        kind: TyKind,
        constraint: InferenceConstraint,
    ) -> Inference {
        let inference = Inference::new_with_constraint(kind, constraint, self.inference_counter);
        self.inference_counter += 1;
        inference
    }
}
