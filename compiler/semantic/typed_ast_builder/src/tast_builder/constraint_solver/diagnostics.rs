use rayc_hash::FxHashSet;
use rayc_solver::instance_resolution::InstanceResolutionError;
use rayc_type::subst::Subst;

use super::{
    CauseID, ConstraintSolver,
    provenance::{ResolvedEffectUnification, ResolvedRootCause},
    solve::{ConstraintError, PendingConstraint},
};
use crate::diagnostic::{
    Diagnostic, EffectUnificationSite, IncompatibleEffectRows, InvalidNumericOperand,
    ResidualSubtype,
};

impl ConstraintSolver {
    fn incompatible_effect_diagnostic(effect_unification: ResolvedEffectUnification) -> Diagnostic {
        let related_sites = effect_unification
            .related_sites
            .into_iter()
            .map(|site| {
                EffectUnificationSite::builder()
                    .effect_row(site.effect_row)
                    .source(site.source)
                    .span(site.span)
                    .build()
            })
            .collect();

        Diagnostic::IncompatibleEffectRows(
            IncompatibleEffectRows::builder()
                .primary_span(effect_unification.span)
                .lesser(effect_unification.lesser)
                .greater(effect_unification.greater)
                .source(effect_unification.source)
                .related_sites(related_sites)
                .build(),
        )
    }

    fn connected_root_ids(
        initial_root_ids: &FxHashSet<CauseID>,
        failed_root_ids: &[FxHashSet<CauseID>],
    ) -> FxHashSet<CauseID> {
        let mut connected = initial_root_ids.clone();

        // A failed constraint whose provenance overlaps this component can introduce
        // more roots that overlap another failed constraint. Close the component
        // transitively so diagnostic coverage does not depend on iteration order.
        loop {
            let previous_len = connected.len();
            for root_ids in failed_root_ids {
                if !connected.is_disjoint(root_ids) {
                    connected.extend(root_ids.iter().copied());
                }
            }

            if connected.len() == previous_len {
                return connected;
            }
        }
    }

    async fn effect_unification_diagnostics(
        &self,
        failed_constraints: &[&PendingConstraint],
        failed_root_ids: &[FxHashSet<CauseID>],
        unreported_roots: &mut FxHashSet<CauseID>,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (pending_constraint, root_ids) in failed_constraints.iter().zip(failed_root_ids.iter())
        {
            let primary_root_id =
                self.provenance.primary_root_cause_id(pending_constraint.cause_id());
            if !unreported_roots.contains(&primary_root_id) {
                continue;
            }

            let connected_root_ids = Self::connected_root_ids(root_ids, failed_root_ids);
            let ResolvedRootCause::EffectUnification(effect_unification) = self
                .provenance
                .resolved_root_cause(primary_root_id, connected_root_ids, &self.solver)
                .await
            else {
                continue;
            };

            for root_id in &effect_unification.root_ids {
                unreported_roots.remove(root_id);
            }
            diags.push(Self::incompatible_effect_diagnostic(effect_unification));
        }
    }

    async fn fallback_diagnostics(
        &self,
        failed_constraints: &[&PendingConstraint],
        failed_root_ids: &[FxHashSet<CauseID>],
        unreported_roots: &mut FxHashSet<CauseID>,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (pending_constraint, root_ids) in failed_constraints.iter().zip(failed_root_ids.iter())
        {
            let primary_root_id =
                self.provenance.primary_root_cause_id(pending_constraint.cause_id());
            if !unreported_roots.remove(&primary_root_id) {
                continue;
            }

            match self
                .provenance
                .resolved_root_cause(primary_root_id, root_ids.clone(), &self.solver)
                .await
            {
                ResolvedRootCause::TraitRefCheck(check) => {
                    diags.push(Diagnostic::from(rayc_resolution::Diagnostic::TraitRefCheck(check)));
                }
                ResolvedRootCause::PredicateObligation(predicate) => {
                    diags.push(Diagnostic::from(rayc_resolution::Diagnostic::Predicate(predicate)));
                }
                ResolvedRootCause::InstanceResolve { trait_ref, span } => {
                    // Preserve the structured failure for rendering at the diagnostic boundary.
                    let error =
                        self.error_for_root_cause(primary_root_id).cloned().unwrap_or_else(|| {
                            ConstraintError::InstanceResolve(InstanceResolutionError::NotReady(
                                trait_ref.clone(),
                            ))
                        });
                    diags.push(
                        crate::diagnostic::InstanceResolution::builder()
                            .span(span)
                            .trait_ref(trait_ref)
                            .error(error)
                            .build()
                            .into(),
                    );
                }
                ResolvedRootCause::Subtype { source, span, subtype } => {
                    diags.push(Diagnostic::ResidualSubtype(
                        ResidualSubtype::builder()
                            .source(source)
                            .span(span)
                            .subype(subtype)
                            .build(),
                    ));
                }
                ResolvedRootCause::NumericOperand { operation, operand, span } => {
                    diags.push(Diagnostic::InvalidNumericOperand(
                        InvalidNumericOperand::builder()
                            .operation(operation)
                            .operand(operand)
                            .span(span)
                            .build(),
                    ));
                }
                ResolvedRootCause::EffectUnification(effect_unification) => {
                    diags.push(Self::incompatible_effect_diagnostic(effect_unification));
                }
            }
        }
    }

    #[must_use]
    pub async fn residual_into_diags(self) -> (Vec<Diagnostic>, Subst) {
        let failed_constraints =
            self.constraint_set.failed_pending_constraints().collect::<Vec<_>>();
        let failed_root_ids = failed_constraints
            .iter()
            .map(|pending_constraint| self.provenance.root_cause_ids(pending_constraint.cause_id()))
            .collect::<Vec<_>>();
        let mut unreported_roots = failed_constraints
            .iter()
            .map(|pending_constraint| {
                self.provenance.primary_root_cause_id(pending_constraint.cause_id())
            })
            .collect::<FxHashSet<_>>();
        let mut diags = Vec::new();

        // Specialized effect diagnostics claim every failed primary root in their
        // connected provenance component. The fallback pass then reports each root
        // that has not already been explained by one of those diagnostics.
        self.effect_unification_diagnostics(
            &failed_constraints,
            &failed_root_ids,
            &mut unreported_roots,
            &mut diags,
        )
        .await;
        self.fallback_diagnostics(
            &failed_constraints,
            &failed_root_ids,
            &mut unreported_roots,
            &mut diags,
        )
        .await;
        debug_assert!(unreported_roots.is_empty());

        (diags, self.provenance.into_subst())
    }
}
