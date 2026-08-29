use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::Subst,
    ty::{Primitive, Ty},
};

use super::{ConstraintSolver, provenance::EffectConflict, solve::PendingConstraint};
use crate::diagnostic::{Diagnostic, IncompatibleEffectRows, ResidualSubtype};

impl ConstraintSolver {
    fn incompatible_effect_diagnostic(
        &self,
        pending_constraint: &PendingConstraint,
        engine: &TrackedEngine,
    ) -> Option<Diagnostic> {
        let cause_id = pending_constraint.cause_id();
        let EffectConflict { first_span, first_effect, second_span, second_effect } =
            self.provenance.effect_conflict(cause_id, engine)?;

        Some(Diagnostic::IncompatibleEffectRows(
            IncompatibleEffectRows::builder()
                .first_span(first_span)
                .first_effect(first_effect)
                .second_span(second_span)
                .second_effect(second_effect)
                .build(),
        ))
    }

    fn effect_conflict_diagnostics(&self, diags: &mut Vec<Diagnostic>, engine: &TrackedEngine) {
        for pending_constraint in self.constraint_set.errored_pending_constraints() {
            if let Some(diagnostic) =
                self.incompatible_effect_diagnostic(pending_constraint, engine)
                && !diags.contains(&diagnostic)
            {
                diags.push(diagnostic);
            }
        }
    }

    fn subtype_conflict_diagnostics(&self, diags: &mut Vec<Diagnostic>, engine: &TrackedEngine) {
        diags.extend(
            self.constraint_set
                .failed_cause_ids()
                .map(|cause_id| self.provenance.primary_root_cause_id(cause_id))
                .filter_map(|cause_id| {
                    let (source, span, subtype) =
                        self.provenance.resolved_subtype_origin(cause_id, engine)?;

                    Some(Diagnostic::ResidualSubtype(
                        ResidualSubtype::builder()
                            .source(source)
                            .span(span)
                            .subype(subtype)
                            .build(),
                    ))
                }),
        );
    }

    #[must_use]
    pub fn residual_into_diags(mut self, engine: &TrackedEngine) -> (Vec<Diagnostic>, Subst) {
        let mut diags = Vec::new();

        self.effect_conflict_diagnostics(&mut diags, engine);
        self.subtype_conflict_diagnostics(&mut diags, engine);

        let numeric_inferences = self.constraint_set.numeric_inferences();
        let default_numeric_type = Ty::new_primitive(Primitive::Int32, engine);
        self.provenance.default_unbound_inferences(
            numeric_inferences,
            &default_numeric_type,
            engine,
        );

        (diags, self.provenance.into_subst())
    }
}
