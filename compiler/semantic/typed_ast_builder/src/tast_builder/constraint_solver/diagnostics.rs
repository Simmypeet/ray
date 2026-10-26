use rayc_hash::FxHashSet;
use rayc_semantic_element::deferred_marker::DeferredMarker;
use rayc_solver::{
    explain::{ProofFailure, RequiredBy, UnsatisfiedGoal},
    instance_resolution::InstanceResolutionError,
};
use rayc_type::subst::Subst;

use super::{
    CauseID, ConstraintSolver,
    provenance::{ResolvedEffectUnification, ResolvedRootCause},
    solve::{ConstraintError, PendingConstraint},
};
use crate::diagnostic::{
    ConflictingInstance, Diagnostic, EffectUnificationSite, IncompatibleEffectRows, ResidualSubtype,
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

        // A failed constraint whose provenance overlaps this component can
        // introduce more roots that overlap another failed constraint.
        // Close the component transitively so diagnostic coverage does
        // not depend on iteration order.
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
        failed_cause_ids: &[CauseID],
        failed_root_ids: &[FxHashSet<CauseID>],
        unreported_roots: &mut FxHashSet<CauseID>,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (cause_id, root_ids) in failed_cause_ids.iter().zip(failed_root_ids.iter()) {
            let primary_root_id = self.provenance.primary_root_cause_id(*cause_id);
            if !unreported_roots.contains(&primary_root_id) {
                continue;
            }

            let connected_root_ids = Self::connected_root_ids(root_ids, failed_root_ids);
            let ResolvedRootCause::EffectUnification(effect_unification) = self
                .provenance
                .resolved_root_cause(primary_root_id, &connected_root_ids, &self.solver)
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
        &mut self,
        failed_cause_ids: &[CauseID],
        failed_root_ids: &[FxHashSet<CauseID>],
        unreported_roots: &mut FxHashSet<CauseID>,
        diags: &mut Vec<Diagnostic>,
    ) {
        for (cause_id, root_ids) in failed_cause_ids.iter().zip(failed_root_ids.iter()) {
            let primary_root_id = self.provenance.primary_root_cause_id(*cause_id);
            if !unreported_roots.remove(&primary_root_id) {
                continue;
            }

            match self.provenance.resolved_root_cause(primary_root_id, root_ids, &self.solver).await
            {
                ResolvedRootCause::TraitRefCheck(check) => {
                    diags.push(Diagnostic::from(rayc_resolution::Diagnostic::TraitRefCheck(check)));
                }
                ResolvedRootCause::PredicateObligation(predicate) => {
                    // A marker goal with the final substitution applied is
                    // explained by the chain of premises that makes it fail;
                    // other predicates keep their own report.
                    match self.solver.explain_predicate(predicate.predicate()).await {
                        Some(failure) => {
                            let required_by =
                                RequiredBy::WhereClause { symbol_id: predicate.symbol_id() };
                            diags.push(Diagnostic::UnsatisfiedGoal(UnsatisfiedGoal::new(
                                predicate.span(),
                                required_by,
                                failure,
                            )));
                        }
                        None => diags.push(Diagnostic::from(
                            rayc_resolution::Diagnostic::Predicate(predicate),
                        )),
                    }
                }
                ResolvedRootCause::InstanceResolve { trait_ref, span } => {
                    // A goal left over when inference ends still has
                    // inference variables, so it is not ready.
                    let error =
                        self.error_for_root_cause(primary_root_id).cloned().unwrap_or_else(|| {
                            ConstraintError::InstanceResolve(InstanceResolutionError::NotReady(
                                trait_ref.clone(),
                            ))
                        });

                    match error {
                        ConstraintError::InstanceResolve(error) => {
                            // A requirement on the error type cannot be
                            // resolved, but the error type only exists
                            // because another diagnostic was already reported
                            // where it came from. For example, a rejected
                            // `coro` has the error type, and the statement
                            // discarding it requires `Drop` of that type.
                            // Reporting the requirement would only repeat the
                            // first error.
                            let Some(failure) =
                                ProofFailure::from_instance_error(trait_ref, &error)
                            else {
                                continue;
                            };

                            diags.push(Diagnostic::UnsatisfiedGoal(UnsatisfiedGoal::new(
                                span,
                                RequiredBy::ImplicitInstance,
                                failure,
                            )));
                        }
                        ConstraintError::TyRelate(error) => {
                            diags.push(
                                ConflictingInstance::builder()
                                    .span(span)
                                    .trait_ref(trait_ref)
                                    .error(error)
                                    .build()
                                    .into(),
                            );
                        }
                    }
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
                ResolvedRootCause::EffectUnification(effect_unification) => {
                    diags.push(Self::incompatible_effect_diagnostic(effect_unification));
                }
            }
        }
    }

    #[must_use]
    pub async fn finish_constraint_solver(
        mut self,
    ) -> (Vec<Diagnostic>, Vec<DeferredMarker>, Subst) {
        let failed_cause_ids = self
            .constraint_set
            .failed_pending_constraints()
            .map(PendingConstraint::cause_id)
            .collect::<Vec<_>>();
        let failed_root_ids = failed_cause_ids
            .iter()
            .map(|cause_id| self.provenance.root_cause_ids(*cause_id))
            .collect::<Vec<_>>();
        let mut unreported_roots = failed_cause_ids
            .iter()
            .map(|cause_id| self.provenance.primary_root_cause_id(*cause_id))
            .collect::<FxHashSet<_>>();
        let mut diags = Vec::new();

        // Specialized effect diagnostics claim every failed primary root in
        // their connected provenance component. The fallback pass then
        // reports each root that has not already been explained by one
        // of those diagnostics.
        self.effect_unification_diagnostics(
            &failed_cause_ids,
            &failed_root_ids,
            &mut unreported_roots,
            &mut diags,
        )
        .await;
        self.fallback_diagnostics(
            &failed_cause_ids,
            &failed_root_ids,
            &mut unreported_roots,
            &mut diags,
        )
        .await;
        debug_assert!(unreported_roots.is_empty());

        (diags, self.constraint_set.into_deferred_markers(), self.provenance.into_subst())
    }
}
