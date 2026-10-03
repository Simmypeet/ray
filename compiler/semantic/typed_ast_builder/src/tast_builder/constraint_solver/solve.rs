use bon::Builder;
use qbice::storage::intern::Interned;
use rayc_resolution::{PredicateConstraint, PredicateObligation};
use rayc_solver::{
    instance_resolution::{InstanceResolutionError, InstanceResolutionObligation},
    ty_relate::{self, DerivedConstraint, Step},
};
use rayc_type::{
    constraint::{instance_trait_ref::InstanceTraitRef, ty_relate::TyRelate},
    subst::Subst,
    trait_ref::TraitRef,
    ty::{
        InferenceConstraint, Ty, TyKind,
        inference::{GenInfer, Inference},
        lifetime::Lifetime,
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
            Ok(Step::Subst(_) | Step::Generalized { .. }) => {
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
                // Type inference ignores lifetimes, so the outlives
                // constraints of the selection are dropped.
                let (result, obligations, _) = resolved.into_parts();

                // Instance search returns the predicates contributed by the selected proof
                // tree.
                self.enqueue_instance_resolution_obligations(obligations, cause_id, queued);

                // Relate the requested dictionary after its predicates have been queued. The
                // worklist is LIFO, so this relation is solved before those predicates.
                queued.push(PendingConstraint {
                    constraint: Constraint::TyRelate(TyRelate::new_invariant(instance, result)),
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
                // Type inference ignores lifetimes; the borrow checker
                // re-checks outlives on the IR.
                PredicateConstraint::Outlives(_) => continue,
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
        // Type inference ignores lifetimes, so the outlives constraints of the
        // step are dropped; the borrow checker re-infers them on the IR.
        let step = self.constraint_solver.solver.entail_ty_relate(&ty_relate).await;
        match step.map(|entailment| entailment.into_parts().0) {
            Ok(Step::Derived(constrs)) => {
                queued.extend(
                    constrs.into_iter().map(|x| self.register_derived_constraint(cause_id, x)),
                );
            }

            Ok(Step::Subst(subst)) => self.apply_step_subst(&subst, cause_id, queued),

            Ok(Step::Generalized { subst, derived }) => {
                self.apply_step_subst(&subst, cause_id, queued);
                queued.extend(
                    derived.into_iter().map(|x| self.register_derived_constraint(cause_id, x)),
                );
            }

            // The relation was normalized, so it waits for a binding.
            Ok(Step::NoProgress) => {
                self.constraint_solver.constraint_set.residual_constraints.push(
                    PendingConstraint { constraint: Constraint::TyRelate(ty_relate), cause_id },
                );
            }

            Err(err) => {
                self.constraint_solver.constraint_set.errored_constraints.push((
                    ConstraintError::TyRelate(err),
                    PendingConstraint { constraint: Constraint::TyRelate(ty_relate), cause_id },
                ));
            }
        }
    }

    /// Composes a substitution produced for the constraint with `cause_id`
    /// and applies it to every pending constraint.
    fn apply_step_subst(
        &mut self,
        subst: &Subst,
        cause_id: CauseID,
        queued: &mut Vec<PendingConstraint>,
    ) {
        self.constraint_solver.provenance.compose_subst(subst, cause_id, &self.engine);
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

        // Effect rows are defaulted after numerics: retrying the residuals
        // after numeric defaulting can still bind them, e.g. when a `Def`
        // requirement fixes a closure's effect.
        self.default_effect_rows().await;

        // No residual is retried from here on: whatever remains is an error,
        // except relations between two variables that wait for a binding.
        self.bind_waiting_variables().await;

        // Lifetimes are defaulted last, because retrying the residuals above
        // can generalize types and create new lifetime inference variables.
        self.default_lifetimes().await;
    }

    /// Binds the pairs of inference variables whose non-invariant relation,
    /// such as `?x <: ?y`, still waits for one of them to be bound.
    ///
    /// Such a relation waits so that the variable bound second is bound to a
    /// generalization of the first one; see `Solver::entail_ty_relate`. Once
    /// the constraints reach a fixed point, nothing else binds either
    /// variable, so each pair is related invariantly instead, which binds the
    /// variables if their inference constraints allow it.
    ///
    /// This is a single pass: no other residual constraint is retried after
    /// these bindings, so every other residual constraint stays an error.
    async fn bind_waiting_variables(&mut self) {
        let residual =
            std::mem::take(&mut self.constraint_solver.constraint_set.residual_constraints);

        for pending in residual {
            let Some(unified) = pending.constraint.waiting_variable_pair() else {
                self.constraint_solver.constraint_set.residual_constraints.push(pending);
                continue;
            };

            // Type inference ignores lifetimes, so the outlives constraints of
            // the step are dropped.
            let step = self.constraint_solver.solver.entail_ty_relate(&unified).await;
            match step.map(|entailment| entailment.into_parts().0) {
                Ok(Step::Subst(subst)) => {
                    let cause_id = pending.cause_id;
                    self.constraint_solver.provenance.compose_subst(&subst, cause_id, &self.engine);
                }

                // An earlier binding already made the variables the same.
                Ok(Step::Derived(derived)) if derived.is_empty() => {}

                Ok(Step::Derived(_) | Step::Generalized { .. } | Step::NoProgress) => {
                    self.constraint_solver.constraint_set.residual_constraints.push(pending);
                }

                Err(error) => {
                    self.constraint_solver
                        .constraint_set
                        .errored_constraints
                        .push((ConstraintError::TyRelate(error), pending));
                }
            }
        }
    }

    /// Erases every lifetime inference variable.
    ///
    /// Lifetime inference variables are never bound. Type inference ignores
    /// lifetimes, and the borrow checker re-infers them on the IR, so they
    /// can take any lifetime. They never keep a constraint from being solved,
    /// so no residual is retried.
    async fn default_lifetimes(&mut self) {
        let lifetimes = self.constraint_solver.take_recorded_lifetime_inferences();

        let erased = Ty::new_lifetime(Lifetime::Erased, &self.engine);
        self.constraint_solver
            .provenance
            .default_unbound_inferences(
                lifetimes,
                |_| Some(erased.clone()),
                &self.constraint_solver.solver,
            )
            .await;
    }

    /// Defaults every numeric type that no constraint determined, then
    /// retries the residual constraints it may unblock.
    ///
    /// Each type defaults according to the inference constraint of its latest
    /// representative: `int32` for a numeric or signed numeric type, and
    /// `float64` for a floating-point type.
    async fn default_numerics(&mut self) {
        let numerics = self.constraint_solver.take_recorded_numeric_inferences();

        let engine = self.engine.clone();
        self.constraint_solver
            .provenance
            .default_unbound_inferences(
                numerics,
                |inference| {
                    inference
                        .constraint()
                        .default_primitive()
                        .map(|primitive| Ty::new_primitive(primitive, &engine))
                },
                &self.constraint_solver.solver,
            )
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

    pub fn new_floating_point_type_inference(&mut self) -> Interned<Ty> {
        let inference = self.gen_infer(TyKind::Star, InferenceConstraint::FloatingPoint);
        self.engine.intern(Ty::Inference(inference))
    }

    pub fn new_equality_comparable_type_inference(&mut self) -> Interned<Ty> {
        let inference = self.gen_infer(TyKind::Star, InferenceConstraint::EqualityComparable);
        self.engine.intern(Ty::Inference(inference))
    }
}
