use qbice::storage::intern::Interned;
use rayc_hash::FxHashSet;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::DerivationRule,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::{Primitive, Ty},
};

use super::{
    Cause, CauseID, ConstraintSolver, ExplanationRule, PendingConstraint, RootCauseOrigin,
};
use crate::diagnostic::{Diagnostic, IncompatibleEffectInstantiations, ResidualSubtype};

impl ConstraintSolver {
    fn group_constraints_by_primary_root_cause(
        &self,
        cause_ids: impl IntoIterator<Item = CauseID>,
    ) -> FxHashSet<CauseID> {
        let mut root_cause_ids = FxHashSet::default();

        for cause_id in cause_ids {
            root_cause_ids.insert(self.primary_root_cause_id(cause_id));
        }

        root_cause_ids
    }

    fn primary_root_cause_id(&self, mut cause_id: CauseID) -> CauseID {
        loop {
            match &self.causes[cause_id] {
                Cause::Root(_root) => return cause_id,
                Cause::Derived(derived_cause) => {
                    cause_id = *derived_cause
                        .parent_causes
                        .first()
                        .expect("a derived cause should have a parent cause");
                }
            }
        }
    }

    fn collect_root_cause_ids(&self, cause_id: CauseID, roots: &mut FxHashSet<CauseID>) {
        match self.causes.get(cause_id) {
            Some(Cause::Root(_root)) => {
                roots.insert(cause_id);
            }
            Some(Cause::Derived(derived_cause)) => {
                for parent_cause in &derived_cause.parent_causes {
                    self.collect_root_cause_ids(*parent_cause, roots);
                }
            }
            None => {}
        }
    }

    fn find_effect_symbol_in_cause(&self, cause_id: CauseID) -> Option<GlobalSymbolID> {
        match self.causes.get(cause_id) {
            Some(Cause::Root(_root)) => None,
            Some(Cause::Derived(derived_cause)) => {
                match derived_cause.rule {
                    ExplanationRule::ConstraintDerivation(
                        DerivationRule::EffectLabelArgumentMatching {
                            effect_symbol_id,
                            argument_index: _argument_index,
                        },
                    ) => return Some(effect_symbol_id),
                    ExplanationRule::ConstraintDerivation(
                        DerivationRule::TypeApplicationMatching,
                    )
                    | ExplanationRule::AppliedSubstitution => {}
                }

                derived_cause
                    .parent_causes
                    .iter()
                    .find_map(|parent| self.find_effect_symbol_in_cause(*parent))
            }
            None => None,
        }
    }

    fn effect_introduction_sites(
        &self,
        cause_id: CauseID,
        effect_symbol_id: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Vec<(RelativeSpan, Interned<Ty>)> {
        let mut roots = FxHashSet::default();
        self.collect_root_cause_ids(cause_id, &mut roots);
        let mut sites = Vec::new();

        for root_id in roots {
            let Cause::Root(root) = &self.causes[root_id] else {
                continue;
            };
            let RootCauseOrigin::EffectIntroduction(origin) = &root.origin else {
                continue;
            };

            let mut introduced_effect =
                origin.introduced_effect.apply_subst_or_clone(&self.subst, engine);
            while let Some(reduced) = introduced_effect.reduce(engine) {
                introduced_effect = reduced;
            }
            let Ty::EffectRow(effect_row) = &*introduced_effect else {
                continue;
            };

            sites.extend(
                effect_row
                    .labels()
                    .filter(|label| label.effect_symbol_id() == effect_symbol_id)
                    .map(|label| (origin.span, Ty::new_effect_row([label.clone()], None, engine))),
            );
        }

        sites.sort_by_key(|(span, _)| *span);
        sites.dedup();
        sites
    }

    fn incompatible_effect_diagnostic(
        &self,
        pending_constraint: &PendingConstraint,
        engine: &TrackedEngine,
    ) -> Option<Diagnostic> {
        let effect_symbol_id = self.find_effect_symbol_in_cause(pending_constraint.cause_id)?;
        let sites =
            self.effect_introduction_sites(pending_constraint.cause_id, effect_symbol_id, engine);

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
        let root_cause_ids = self.group_constraints_by_primary_root_cause(
            self.residual_constraints
                .iter()
                .map(|x| x.cause_id)
                .chain(self.errored_constraints.iter().map(|x| x.1.cause_id)),
        );

        let mut diags = Vec::new();
        for (_, pending) in &self.errored_constraints {
            if let Some(diagnostic) = self.incompatible_effect_diagnostic(pending, engine)
                && !diags.contains(&diagnostic)
            {
                diags.push(diagnostic);
            }
        }
        diags.extend(root_cause_ids.into_iter().filter_map(|cause_id| {
            match &self.causes[cause_id] {
                Cause::Root(root) => match &root.origin {
                    RootCauseOrigin::Subtype(subtype_constraint_origin) => {
                        let subtype = subtype_constraint_origin
                            .original_subtype
                            .apply_subst_or_clone(&self.subst, engine);

                        Some(Diagnostic::ResidualSubtype(
                            ResidualSubtype::builder()
                                .source(subtype_constraint_origin.source)
                                .span(subtype_constraint_origin.span)
                                .subype(subtype)
                                .build(),
                        ))
                    }

                    RootCauseOrigin::EffectIntroduction(_) | RootCauseOrigin::EffectSharing(_) => {
                        None
                    }
                },
                Cause::Derived(_derived) => None,
            }
        }));

        let int32 = Ty::new_primitive(Primitive::Int32, engine);
        let numeric_defaults = self
            .numeric_inferences
            .iter()
            .filter(|inference| self.subst.get(&**inference).is_none())
            .map(|inference| (*inference, int32.clone()))
            .collect();
        self.subst.compose(&numeric_defaults, engine);

        (diags, self.subst)
    }
}
