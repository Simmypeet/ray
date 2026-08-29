use rayc_hash::FxHashSet;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::Subst,
    ty::{Primitive, Ty},
};

use super::{ConstraintSolver, SubtypeSource, solve::PendingConstraint};
use crate::diagnostic::{Diagnostic, IncompatibleEffectInstantiations, ResidualSubtype};

impl ConstraintSolver {
    fn incompatible_effect_diagnostic(
        &self,
        pending_constraint: &PendingConstraint,
        engine: &TrackedEngine,
    ) -> Option<Diagnostic> {
        let cause_id = pending_constraint.cause_id();
        let effect_symbol_id = self.provenance.find_effect_symbol_in_cause(cause_id)?;
        let sites = self.provenance.effect_introduction_sites(cause_id, effect_symbol_id, engine);

        for (index, (first_span, first_effect)) in sites.iter().enumerate() {
            for (second_span, second_effect) in &sites[index + 1..] {
                if first_effect == second_effect {
                    continue;
                }

                return Some(Diagnostic::IncompatibleEffectInstantiations(
                    IncompatibleEffectInstantiations::builder()
                        .first_span(*first_span)
                        .first_effect(first_effect.clone())
                        .second_span(*second_span)
                        .second_effect(second_effect.clone())
                        .build(),
                ));
            }
        }

        None
    }

    #[must_use]
    pub fn residual_into_diags(mut self, engine: &TrackedEngine) -> (Vec<Diagnostic>, Subst) {
        let primary_root_cause_ids = self
            .constraint_set
            .failed_cause_ids()
            .map(|cause_id| self.provenance.primary_root_cause_id(cause_id))
            .collect::<FxHashSet<_>>();

        let mut diags = Vec::new();
        for pending_constraint in self.constraint_set.errored_pending_constraints() {
            if let Some(diagnostic) =
                self.incompatible_effect_diagnostic(pending_constraint, engine)
                && !diags.contains(&diagnostic)
            {
                diags.push(diagnostic);
            }
        }
        let has_incompatible_effect_diagnostic = !diags.is_empty();

        diags.extend(primary_root_cause_ids.into_iter().filter_map(|cause_id| {
            let (source, span, subtype) =
                self.provenance.resolved_subtype_origin(cause_id, engine)?;

            // An incompatible-effect diagnostic already identifies the precise introduction
            // sites. A body/signature mismatch from the same failed composition is only a
            // cascading summary of that error.
            if source == SubtypeSource::FunctionBodyEffect && has_incompatible_effect_diagnostic {
                return None;
            }

            Some(Diagnostic::ResidualSubtype(
                ResidualSubtype::builder().source(source).span(span).subype(subtype).build(),
            ))
        }));

        let numeric_inferences = self.constraint_set.numeric_inferences().collect::<Vec<_>>();
        let default_numeric_type = Ty::new_primitive(Primitive::Int32, engine);
        self.provenance.default_unbound_inferences(
            numeric_inferences,
            &default_numeric_type,
            engine,
        );

        (diags, self.provenance.into_subst())
    }
}
