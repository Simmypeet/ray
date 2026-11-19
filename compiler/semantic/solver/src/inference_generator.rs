//! Allocation of inference variables, pluggable so a caller can observe
//! every variable a solver creates.

use std::{any::Any, fmt::Debug};

use rayc_type::ty::{InferenceConstraint, TyKind, inference::Inference};

/// Creates the inference variables of one solver, each distinct from all
/// previously generated ones.
pub trait InferenceGenerator: Any + Debug + Send + Sync {
    fn generate(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference;

    fn as_any(&self) -> &dyn Any;

    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// Numbers inference variables sequentially; not `Copy`, since a copy would
/// hand out the same variables again.
#[allow(missing_copy_implementations)]
#[derive(Debug, Default)]
pub struct CountingInferenceGenerator {
    counter: u64,
}

impl InferenceGenerator for CountingInferenceGenerator {
    fn generate(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        let inference = Inference::new_with_constraint(kind, constraint, self.counter);
        self.counter += 1;
        inference
    }

    fn as_any(&self) -> &dyn Any { self }

    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}
