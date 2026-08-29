use rayc_hash::FxHashSet;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::Constraint,
    subst::{Subst, Substitutable},
    ty::inference::Inference,
};

use super::{Cause, CauseID, ConstraintSolver, DerivedCause, ExplanationRule, PendingConstraint};

impl ConstraintSolver {
    pub(super) fn insert_derived_cause(
        &mut self,
        rule: ExplanationRule,
        mut parent_causes: Vec<CauseID>,
    ) -> CauseID {
        let mut seen = FxHashSet::default();
        parent_causes.retain(|cause_id| seen.insert(*cause_id));
        self.causes.insert(Cause::Derived(DerivedCause { rule, parent_causes }))
    }

    fn constraint_has_inference_variable(constraint: &Constraint, inference: &Inference) -> bool {
        match constraint {
            Constraint::Subtype(subtype) => {
                subtype.lesser().has_inference_variable(inference)
                    || subtype.greater().has_inference_variable(inference)
            }
        }
    }

    fn apply_subst_with_causes(
        &mut self,
        pending_constraint: &PendingConstraint,
        subst: &Subst,
        binding_causes: &[(Inference, CauseID)],
        engine: &TrackedEngine,
    ) -> Option<PendingConstraint> {
        // Example:
        //
        //   pending constraint B: {State[bool] | ?b} ~ ?parent
        //   known binding A:      ?parent := {State[int32] | ?a}
        //
        // Applying A produces `{State[bool] | ?b} ~ {State[int32] | ?a}`.
        // Its explanation must be `AppliedSubstitution(B, A)`: B tells us where
        // `State[bool]` came from, while A leads back to `State[int32]`.
        let constraint = pending_constraint.constraint.apply_subst(subst, engine)?;
        let mut parent_causes = vec![pending_constraint.cause_id];

        // Only attach causes for bindings that occur in this constraint. Other
        // entries in the substitution are unrelated and would add diagnostic
        // noise if they were included.
        parent_causes.extend(binding_causes.iter().filter_map(|(inference, cause_id)| {
            Self::constraint_has_inference_variable(&pending_constraint.constraint, inference)
                .then_some(*cause_id)
        }));
        let cause_id =
            self.insert_derived_cause(ExplanationRule::AppliedSubstitution, parent_causes);

        Some(PendingConstraint { constraint, cause_id })
    }

    pub(super) fn apply_subst(
        &mut self,
        pending_constraint: &PendingConstraint,
        subst: &Subst,
        binding_cause: CauseID,
        engine: &TrackedEngine,
    ) -> Option<PendingConstraint> {
        // Every mapping in this newly produced substitution came from the same
        // solver step, and therefore has the same cause.
        let binding_causes = subst
            .inference_mappings()
            .map(|(inference, _)| (inference, binding_cause))
            .collect::<Vec<_>>();
        self.apply_subst_with_causes(pending_constraint, subst, &binding_causes, engine)
    }

    pub(super) fn apply_current_subst_or_original(
        &mut self,
        pending_constraint: PendingConstraint,
        engine: &TrackedEngine,
    ) -> PendingConstraint {
        // Normalize newly enqueued constraints here, where applying the current
        // substitution can also attach provenance. Applying `latest_type` before
        // creating the PendingConstraint would change the type but lose its cause.
        let subst = self.subst.clone();
        let binding_causes = subst
            .inference_mappings()
            .filter_map(|(inference, _)| {
                self.subst_causes.get(&inference).map(|cause_id| (inference, *cause_id))
            })
            .collect::<Vec<_>>();

        self.apply_subst_with_causes(&pending_constraint, &subst, &binding_causes, engine)
            .unwrap_or(pending_constraint)
    }

    pub(super) fn compose_subst(
        &mut self,
        subst: &Subst,
        binding_cause: CauseID,
        engine: &TrackedEngine,
    ) {
        // Provenance must compose along with types. If the existing substitution
        // is `?a := ?b` with cause A, and this solver step adds `?b := T` with
        // cause B, the composed `?a := T` depends on both A and B.
        let new_inferences =
            subst.inference_mappings().map(|(inference, _)| inference).collect::<Vec<_>>();
        let existing_bindings = self
            .subst
            .inference_mappings()
            .filter_map(|(inference, ty)| {
                self.subst_causes.get(&inference).map(|cause_id| (inference, ty.clone(), *cause_id))
            })
            .collect::<Vec<_>>();

        for (inference, ty, existing_cause) in existing_bindings {
            if new_inferences.iter().any(|new_inference| ty.has_inference_variable(new_inference)) {
                let cause_id =
                    self.insert_derived_cause(ExplanationRule::AppliedSubstitution, vec![
                        existing_cause,
                        binding_cause,
                    ]);
                self.subst_causes.insert(inference, cause_id);
            }
        }

        for inference in new_inferences {
            self.subst_causes.entry(inference).or_insert(binding_cause);
        }
        self.subst.compose(subst, engine);
    }
}
