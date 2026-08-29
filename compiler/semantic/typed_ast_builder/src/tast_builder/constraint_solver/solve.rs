use qbice::storage::intern::Interned;
use rayc_arena::Arena;
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{DerivedConstraint, Step},
    reduce::Reduce,
    solver::Solver,
    subst::Subst,
    ty::{
        InferenceConstraint, Ty, TyKind,
        inference::{GenInfer, Inference},
    },
};

use super::{CauseID, ConstraintSolver, ExplanationRule, PendingConstraint};
use crate::tast_builder::TAstBuilder;

impl Reduce for PendingConstraint {
    fn reduce(&self, engine: &TrackedEngine) -> Option<Self>
    where
        Self: Sized,
    {
        self.constraint
            .reduce(engine)
            .map(|new_constraint| Self { constraint: new_constraint, cause_id: self.cause_id })
    }
}

impl ConstraintSolver {
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self {
        Self {
            causes: Arena::default(),
            residual_constraints: Vec::new(),
            errored_constraints: Vec::new(),
            numeric_inferences: Vec::new(),
            subst: Subst::new_empty(),
            subst_causes: FxHashMap::default(),
            solver: Solver::new(engine),
        }
    }
}

impl TAstBuilder {
    pub(super) fn push_constraint(&mut self, constr: PendingConstraint) {
        self.push_constraints(vec![constr]);
    }

    fn register_derived_constraint(
        &mut self,
        parent_cause: CauseID,
        derived_constraint: DerivedConstraint,
    ) -> PendingConstraint {
        let cause_id = self.constraint_solver.insert_derived_cause(
            ExplanationRule::ConstraintDerivation(derived_constraint.rule),
            vec![parent_cause],
        );

        PendingConstraint { constraint: derived_constraint.constraint, cause_id }
    }

    pub(super) fn push_constraints(&mut self, queued: Vec<PendingConstraint>) {
        let mut normalized = Vec::with_capacity(queued.len());
        for pending_constraint in queued {
            normalized.push(
                self.constraint_solver
                    .apply_current_subst_or_original(pending_constraint, &self.engine),
            );
        }
        let mut queued = normalized;

        while let Some(pending_constraint) = queued.pop() {
            match self.constraint_solver.solver.entail(&pending_constraint.constraint) {
                Ok(Step::Derived(constrs)) => {
                    queued.extend(
                        constrs.into_iter().map(|x| {
                            self.register_derived_constraint(pending_constraint.cause_id, x)
                        }),
                    );
                }

                Ok(Step::Subst(subst)) => {
                    let binding_cause = pending_constraint.cause_id;
                    self.move_constraints_from_residual(&subst, binding_cause, &mut queued);

                    for queued_constraint in &mut queued {
                        if let Some(new_constraint) = self.constraint_solver.apply_subst(
                            queued_constraint,
                            &subst,
                            binding_cause,
                            &self.engine,
                        ) {
                            *queued_constraint = new_constraint;
                        }
                    }

                    self.constraint_solver.compose_subst(&subst, binding_cause, &self.engine);
                }

                Ok(Step::NoProgress) => {
                    if let Some(reduced_constraint) = pending_constraint.reduce(&self.engine) {
                        queued.push(reduced_constraint);
                    } else {
                        self.constraint_solver.residual_constraints.push(pending_constraint);
                    }
                }

                Err(err) => {
                    self.constraint_solver.errored_constraints.push((err, pending_constraint));
                }
            }
        }
    }

    fn move_constraints_from_residual(
        &mut self,
        subst: &Subst,
        binding_cause: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        let mut i = 0;

        while i < self.constraint_solver.residual_constraints.len() {
            let pending_constraint = self.constraint_solver.residual_constraints[i].clone();
            let new_constraint = self.constraint_solver.apply_subst(
                &pending_constraint,
                subst,
                binding_cause,
                &self.engine,
            );

            match new_constraint {
                Some(new_constraint) => {
                    queued.push(new_constraint);
                    self.constraint_solver.residual_constraints.remove(i);
                }
                None => {
                    i += 1;
                }
            }
        }
    }
}

impl GenInfer for ConstraintSolver {
    fn gen_infer(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        if kind == TyKind::Star && constraint == InferenceConstraint::Numeric {
            let inference = self.solver.new_inference_with_constraint(kind, constraint);
            self.numeric_inferences.push(inference);
            inference
        } else {
            self.solver.new_inference_with_constraint(kind, constraint)
        }
    }
}

impl TAstBuilder {
    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::Star)
    }

    pub fn new_type_inference_with_kind(&mut self, kind: TyKind) -> Interned<Ty> {
        let inference = self.constraint_solver.gen_infer(kind, InferenceConstraint::Any);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_numeric_type_inference(&mut self) -> Interned<Ty> {
        let inference =
            self.constraint_solver.gen_infer(TyKind::Star, InferenceConstraint::Numeric);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_equality_comparable_type_inference(&mut self) -> Interned<Ty> {
        let inference =
            self.constraint_solver.gen_infer(TyKind::Star, InferenceConstraint::EqualityComparable);
        self.engine.intern(Ty::Inference(inference))
    }
}
