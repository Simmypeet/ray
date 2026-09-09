use bon::Builder;
use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_solver::{
    instance_resolution::InstanceResolutionError,
    ty_relate::{self, DerivedConstraint, Step},
};
use rayc_type::{
    constraint::{instance_trait_ref::InstanceTraitRef, ty_relate::TyRelate},
    reduce::Reduce,
    trait_ref::TraitRef,
    ty::{
        InferenceConstraint, Ty, TyKind,
        inference::{GenInfer, Inference},
    },
};

use super::{CauseID, ConstraintSolver};
use crate::tast_builder::{TAstBuilder, constraint_solver::constraints::Constraint};

#[derive(Debug)]
pub struct ConstraintSet {
    residual_constraints: Vec<PendingConstraint>,
    errored_constraints: Vec<(ConstraintError, PendingConstraint)>,
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

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, qbice::StableHash, qbice::Encode, qbice::Decode,
)]
pub enum ConstraintError {
    TyRelate(ty_relate::Error),
    InstanceResolve(InstanceResolutionError),
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
    async fn reduce(
        &self,
        engine: &TrackedEngine,
        givens: &[rayc_type::where_clause::PredicateKind],
    ) -> Option<Self>
    where
        Self: Sized,
    {
        self.constraint
            .reduce(engine, givens)
            .await
            .map(|new_constraint| Self { constraint: new_constraint, cause_id: self.cause_id })
    }
}

impl TAstBuilder {
    pub(super) async fn push_constraint(&mut self, constr: PendingConstraint) {
        self.push_constraints(vec![constr]).await;
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

        PendingConstraint {
            constraint: Constraint::TyRelate(derived_constraint.ty_relate),
            cause_id,
        }
    }

    pub(in crate::tast_builder) async fn push_constraints(
        &mut self,
        mut queued: Vec<PendingConstraint>,
    ) {
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
            match pending_constraint.constraint {
                Constraint::InstanceResolve { instance, trait_ref } => {
                    self.entail_instance_resolve(
                        instance,
                        trait_ref,
                        pending_constraint.cause_id,
                        &mut queued,
                    )
                    .await;
                }
                Constraint::InstanceTraitRef(check) => {
                    self.entail_instance_trait_ref(check, pending_constraint.cause_id, &mut queued)
                        .await;
                }
                Constraint::TyRelate(ty_relate) => {
                    self.entail_relate(ty_relate, pending_constraint.cause_id, &mut queued).await;
                }
            }
        }
    }

    async fn entail_instance_trait_ref(
        &mut self,
        check: InstanceTraitRef,
        cause_id: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        match self.constraint_solver.solver.entail_instance_trait_ref(&check).await {
            Ok(Step::Derived(derived)) => queued.extend(
                derived
                    .into_iter()
                    .map(|derived| self.register_derived_constraint(cause_id, derived)),
            ),
            Ok(Step::NoProgress) => {
                self.constraint_solver.constraint_set.residual_constraints.push(
                    PendingConstraint { constraint: Constraint::InstanceTraitRef(check), cause_id },
                );
            }
            Err(error) => self.constraint_solver.constraint_set.errored_constraints.push((
                ConstraintError::TyRelate(error),
                PendingConstraint { constraint: Constraint::InstanceTraitRef(check), cause_id },
            )),
            Ok(Step::Subst(_)) => {
                unreachable!("trait checks only derive type relations")
            }
        }
    }

    async fn entail_instance_resolve(
        &mut self,
        instance: Interned<Ty>,
        trait_ref: TraitRef,
        cause_id: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        match self.constraint_solver.solver.resolve_instance(trait_ref.clone()).await {
            Ok(result) => {
                let cause_id =
                    self.constraint_solver.provenance.insert_instance_resolution_cause(cause_id);
                queued.push(PendingConstraint {
                    constraint: Constraint::TyRelate(TyRelate::new(instance, result)),
                    cause_id,
                });
            }
            Err(InstanceResolutionError::NotReady(_)) => {
                self.constraint_solver.constraint_set.residual_constraints.push(
                    PendingConstraint {
                        constraint: Constraint::InstanceResolve { instance, trait_ref },
                        cause_id,
                    },
                );
            }
            Err(error) => {
                self.constraint_solver.constraint_set.errored_constraints.push((
                    ConstraintError::InstanceResolve(error),
                    PendingConstraint {
                        constraint: Constraint::InstanceResolve { instance, trait_ref },
                        cause_id,
                    },
                ));
            }
        }
    }

    async fn entail_relate(
        &mut self,
        ty_relate: TyRelate,
        cause_id: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        match self.constraint_solver.solver.entail_ty_relate(&ty_relate).await {
            Ok(Step::Derived(constrs)) => {
                queued.extend(
                    constrs.into_iter().map(|x| self.register_derived_constraint(cause_id, x)),
                );
            }

            Ok(Step::Subst(subst)) => {
                self.constraint_solver.provenance.compose_subst(&subst, cause_id, &self.engine);
                self.move_constraints_from_residual(queued);

                for queued_constraint in queued {
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
                if let Some(reduced_constraint) =
                    ty_relate.reduce(&self.engine, self.constraint_solver.solver.givens()).await
                {
                    queued.push(PendingConstraint {
                        constraint: Constraint::TyRelate(reduced_constraint),
                        cause_id,
                    });
                } else {
                    self.constraint_solver.constraint_set.residual_constraints.push(
                        PendingConstraint { constraint: Constraint::TyRelate(ty_relate), cause_id },
                    );
                }
            }

            Err(err) => {
                self.constraint_solver.constraint_set.errored_constraints.push((
                    ConstraintError::TyRelate(err),
                    PendingConstraint { constraint: Constraint::TyRelate(ty_relate), cause_id },
                ));
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

impl ConstraintSolver {
    pub(super) fn error_for_root_cause(&self, root_cause_id: CauseID) -> Option<&ConstraintError> {
        self.constraint_set.errored_constraints.iter().find_map(|(error, pending)| {
            (self.provenance.primary_root_cause_id(pending.cause_id()) == root_cause_id)
                .then_some(error)
        })
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

impl GenInfer for TAstBuilder {
    fn gen_infer(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        self.constraint_solver.gen_infer(kind, constraint)
    }
}

impl TAstBuilder {
    pub async fn finish_constraints(&mut self) {
        let numeric =
            self.constraint_solver.constraint_set.numeric_inferences().collect::<Vec<_>>();

        let default = Ty::new_primitive(rayc_type::ty::Primitive::Int32, &self.engine);
        self.constraint_solver
            .provenance
            .default_unbound_inferences(numeric, &default, &self.constraint_solver.solver)
            .await;
        let mut queued = Vec::new();
        self.move_constraints_from_residual(&mut queued);
        self.push_constraints(queued).await;
    }

    pub fn new_type_inference(&mut self) -> Interned<Ty> {
        self.new_type_inference_with_kind(TyKind::Star)
    }

    pub fn new_type_inference_with_kind(&mut self, kind: TyKind) -> Interned<Ty> {
        let inference = self.gen_infer(kind, InferenceConstraint::Any);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_numeric_type_inference(&mut self) -> Interned<Ty> {
        let inference = self.gen_infer(TyKind::Star, InferenceConstraint::Numeric);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_equality_comparable_type_inference(&mut self) -> Interned<Ty> {
        let inference = self.gen_infer(TyKind::Star, InferenceConstraint::EqualityComparable);
        self.engine.intern(Ty::Inference(inference))
    }
}
