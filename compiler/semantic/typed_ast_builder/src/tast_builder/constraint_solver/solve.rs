use bon::Builder;
use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{self, Constraint, DerivedConstraint, Step},
    reduce::Reduce,
    ty::{
        InferenceConstraint, Ty, TyKind,
        inference::{GenInfer, Inference},
    },
};

use super::{CauseID, ConstraintSolver};
use crate::tast_builder::TAstBuilder;

#[derive(Debug)]
pub struct ConstraintSet {
    residual_constraints: Vec<PendingConstraint>,
    errored_constraints: Vec<(constraint::Error, PendingConstraint)>,
    numeric_inferences: Vec<Inference>,
}

impl ConstraintSet {
    pub const fn new() -> Self {
        Self {
            residual_constraints: Vec::new(),
            errored_constraints: Vec::new(),
            numeric_inferences: Vec::new(),
        }
    }

    pub(super) fn failed_pending_constraints(&self) -> impl Iterator<Item = &PendingConstraint> {
        self.errored_constraints
            .iter()
            .map(|(_, pending)| pending)
            .chain(self.residual_constraints.iter())
    }

    pub(super) fn numeric_inferences(&self) -> impl Iterator<Item = Inference> + '_ {
        self.numeric_inferences.iter().copied()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct PendingConstraint {
    constraint: Constraint,
    cause_id: CauseID,
}

impl PendingConstraint {
    pub const fn cause_id(&self) -> CauseID { self.cause_id }

    pub const fn constraint(&self) -> &Constraint { &self.constraint }

    pub fn interned_recursive_iter(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.constraint.interned_recursive_iter()
    }
}

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

impl TAstBuilder {
    pub(super) fn push_constraint(&mut self, constr: PendingConstraint) {
        self.push_constraints(vec![constr]);
    }

    fn register_derived_constraint(
        &mut self,
        parent_cause: CauseID,
        derived_constraint: DerivedConstraint,
    ) -> PendingConstraint {
        let cause_id = self
            .constraint_solver
            .provenance
            .insert_derivation_cause(derived_constraint.rule, parent_cause);

        PendingConstraint { constraint: derived_constraint.constraint, cause_id }
    }

    pub(super) fn push_constraints(&mut self, mut queued: Vec<PendingConstraint>) {
        // make sure the new constraints are updated with the latest substitution before
        // we start processing them
        for queued in &mut queued {
            if let Some(new) =
                self.constraint_solver.provenance.apply_subst_with_causes(queued, &self.engine)
            {
                *queued = new;
            }
        }

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
                    self.constraint_solver.provenance.compose_subst(
                        &subst,
                        pending_constraint.cause_id,
                        &self.engine,
                    );
                    self.move_constraints_from_residual(&mut queued);

                    for queued_constraint in &mut queued {
                        if let Some(new_constraint) = self
                            .constraint_solver
                            .provenance
                            .apply_subst_with_causes(queued_constraint, &self.engine)
                        {
                            *queued_constraint = new_constraint;
                        }
                    }
                }

                Ok(Step::NoProgress) => {
                    if let Some(reduced_constraint) = pending_constraint.reduce(&self.engine) {
                        queued.push(reduced_constraint);
                    } else {
                        self.constraint_solver
                            .constraint_set
                            .residual_constraints
                            .push(pending_constraint);
                    }
                }

                Err(err) => {
                    self.constraint_solver
                        .constraint_set
                        .errored_constraints
                        .push((err, pending_constraint));
                }
            }
        }
    }

    fn move_constraints_from_residual(&mut self, queued: &mut Vec<PendingConstraint>) {
        let mut i = 0;

        while i < self.constraint_solver.constraint_set.residual_constraints.len() {
            let new_constraint = self.constraint_solver.provenance.apply_subst_with_causes(
                &self.constraint_solver.constraint_set.residual_constraints[i],
                &self.engine,
            );

            match new_constraint {
                Some(new_constraint) => {
                    queued.push(new_constraint);
                    self.constraint_solver.constraint_set.residual_constraints.remove(i);
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
        let inference = self.solver.new_inference_with_constraint(kind, constraint);
        if kind == TyKind::Star && constraint == InferenceConstraint::Numeric {
            self.constraint_set.numeric_inferences.push(inference);
        }
        inference
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
