use bon::Builder;
use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_resolution::{PredicateConstraint, PredicateObligation};
use rayc_solver::{
    instance_resolution::{InstanceResolutionError, InstanceResolutionObligation},
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
}

impl ConstraintSet {
    pub const fn new() -> Self {
        Self { residual_constraints: Vec::new(), errored_constraints: Vec::new() }
    }

    pub(super) fn failed_pending_constraints(&self) -> impl Iterator<Item = &PendingConstraint> {
        self.errored_constraints
            .iter()
            .map(|(_, pending)| pending)
            .chain(self.residual_constraints.iter())
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
                Constraint::MarkerPredicate(predicate) => {
                    self.entail_marker_predicate(predicate, pending_constraint.cause_id).await;
                }
                Constraint::TyRelate(ty_relate) => {
                    self.entail_relate(ty_relate, pending_constraint.cause_id, &mut queued).await;
                }
            }
        }
    }

    async fn entail_marker_predicate(
        &mut self,
        predicate: rayc_type::where_clause::MarkerPredicate,
        cause_id: CauseID,
    ) {
        if self.constraint_solver.solver.entails_marker_predicate(predicate.clone()).await {
            return;
        }

        // A later type substitution may make an unresolved marker goal
        // provable, so retain it in the common residual worklist.
        self.constraint_solver.constraint_set.residual_constraints.push(PendingConstraint {
            constraint: Constraint::MarkerPredicate(predicate),
            cause_id,
        });
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
            Ok(resolved) => {
                let cause_id =
                    self.constraint_solver.provenance.insert_instance_resolution_cause(cause_id);
                let (result, obligations) = resolved.into_parts();

                // Instance search returns the predicates contributed by the selected proof
                // tree.
                self.enqueue_instance_resolution_obligations(obligations, cause_id, queued);

                // Relate the requested dictionary after its predicates have been queued. The
                // worklist is LIFO, so this relation is solved before those predicates.
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

    fn enqueue_instance_resolution_obligations(
        &mut self,
        obligations: Vec<InstanceResolutionObligation>,
        cause_id: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        let span = self.constraint_solver.provenance.instance_resolution_span(cause_id);

        // Give each returned predicate its own diagnostic root at the implicit-use
        // site.
        for obligation in obligations {
            let (instance_id, predicate) = obligation.into_parts();
            let obligation = PredicateObligation::new(predicate, instance_id, span);
            let constraint = match obligation.constraint() {
                PredicateConstraint::TyRelate(constraint) => Constraint::TyRelate(constraint),
                PredicateConstraint::Marker(marker) => Constraint::MarkerPredicate(marker),
            };
            let predicate_cause_id =
                self.constraint_solver.provenance.insert_root_cause(obligation.clone());

            queued.push(PendingConstraint { constraint, cause_id: predicate_cause_id });
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
        self.solver.new_inference_with_constraint(kind, constraint)
    }
}

impl GenInfer for TAstBuilder {
    fn gen_infer(&mut self, kind: TyKind, constraint: InferenceConstraint) -> Inference {
        self.constraint_solver.gen_infer(kind, constraint)
    }
}

impl TAstBuilder {
    pub async fn finish_constraints(&mut self) {
        self.default_numerics().await;

        // Effect rows are defaulted last: retrying the residuals after numeric
        // defaulting can still bind them, e.g. when a `Def` requirement fixes
        // a closure's effect.
        self.default_effect_rows().await;
    }

    /// Defaults every numeric literal that no constraint determined to
    /// `int32`, then retries the residual constraints it may unblock.
    async fn default_numerics(&mut self) {
        let numerics = self.constraint_solver.take_recorded_numeric_inferences();

        let default = Ty::new_primitive(rayc_type::ty::Primitive::Int32, &self.engine);
        self.constraint_solver
            .provenance
            .default_unbound_inferences(numerics, &default, &self.constraint_solver.solver)
            .await;

        let mut queued = Vec::new();
        self.move_constraints_from_residual(&mut queued);
        self.push_constraints(queued).await;
    }

    /// Closes every effect row that no constraint determines with the empty
    /// row, e.g. the effect of a closure that is never called.
    ///
    /// Effect rows are related only by exact row unification, so after the
    /// constraints reach a fixed point, an unbound row that no residual or
    /// errored constraint mentions can take any value without affecting a
    /// solved constraint. Rows mentioned by those constraints are left alone,
    /// so their diagnostics are reported against the uninferred row.
    async fn default_effect_rows(&mut self) {
        let effect_rows = self.constraint_solver.take_recorded_effect_row_inferences();
        let excluded = self.constraint_solver.provenance.inferences_in(
            self.constraint_solver.constraint_set.failed_pending_constraints(),
            &self.engine,
        );

        let defaulted = self.constraint_solver.provenance.default_unbound_effect_rows(
            effect_rows,
            &excluded,
            &self.engine,
        );
        if !defaulted {
            return;
        }

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
