use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{DerivationRule, subtype::Subtype},
    reduce::Reduce,
    subst::{Subst, Substitutable},
    ty::{Ty, inference::Inference},
};
use rayc_typed_ast::{typed_expr::TypedExprID, typed_function::TypedFunctionLocalID};

use crate::tast_builder::{TAstBuilder, constraint_solver::solve::PendingConstraint};

/// A struct that tracks the **provenance** of constraints and substitutions in
/// the constraint solver.
///
/// The "provenance" describes the **reasons** why a constraint or substitution
/// exists and how it was derived and evolved over time. The primary purpose of
/// this information is to provide **diagnostic explanations** for why a
/// constraint was generated and why it failed to be solved.
///
/// We track the provenance by using [`Cause`]s, which are nodes in a directed
/// acyclic graph (DAG) that represent the derivation history of constraints and
/// substitutions. Each [`Cause`] can be either a **root cause** (the original
/// source of a constraint) or a **derived cause** (a constraint that was
/// derived from one or more parent causes). The DAG structure allows us to
/// trace back the history of a constraint and understand how it came to be in
/// its current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    causes: Arena<Cause>,

    // IMPORTANT INVARIANT: Every inference variable in the substitution must have a corresponding
    // cause in `subst_causes`.
    subst: Subst, /* Yes, the subst belongs here. Since `subst_causes` couples the substitution
                   * with the causes of its inferences, it makes sense to keep them together in
                   * the same struct. */
    subst_causes: FxHashMap<Inference, CauseID>,
}

impl Provenance {
    pub fn new() -> Self {
        Self { causes: Arena::new(), subst: Subst::default(), subst_causes: FxHashMap::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum SubtypeSource {
    FunctionCall,
    LambdaInvocation,
    VariableAssignment,
    BinaryOperator,
    IfCondition,
    IfBranch,
    ReturnType,
    FunctionBodyEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct SubtypeConstraintOrigin {
    original_subtype: Subtype,
    source: SubtypeSource,
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct EffectIntroductionConstraintOrigin {
    expression_id: TypedFunctionLocalID<TypedExprID>,
    span: RelativeSpan,
    introduced_effect: Interned<Ty>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, From)]
pub enum RootCauseOrigin {
    Subtype(SubtypeConstraintOrigin),
    EffectSharing,
    EffectIntroduction(EffectIntroductionConstraintOrigin),
}

/// Describes that the constraint was generated from the source code of the
/// program. Whenever a constraint is pushed into the solver, it will always
/// have a [`RootCause`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootCause {
    origin: RootCauseOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum StepRule {
    Derivation(DerivationStep),
    AppliedSubstitution(AppliedSubstitutionStep),
}

/// The original constraint was simplified into smaller sets but euivalent
/// constraints.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivationStep {
    derivation_rule: DerivationRule,
    parent_cause_id: CauseID,
}

/// The constraint was applied with a substitution
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct AppliedSubstitutionStep {
    /// The original constraint that was rewritten by the substitution.
    original_cause_id: CauseID,

    /// The causes of the bindings in the substitution that were applied to the
    binding_cause_ids: Vec<CauseID>,
}

/// The constraints were generated from an existing constraint.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedCause {
    rule: StepRule,
}

/// Describes how the **constraint** is generated and why it exists.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Root(RootCause),
    Derived(DerivedCause),
}

pub type CauseID = ID<Cause>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectConflict {
    pub first_span: RelativeSpan,
    pub first_effect: Interned<Ty>,
    pub second_span: RelativeSpan,
    pub second_effect: Interned<Ty>,
}

impl Provenance {
    pub fn insert_root_cause(&mut self, root_cause: impl Into<RootCauseOrigin>) -> CauseID {
        self.causes.insert(Cause::Root(RootCause { origin: root_cause.into() }))
    }

    pub fn insert_derivation_cause(
        &mut self,
        derivation_rule: DerivationRule,
        parent_cause_id: CauseID,
    ) -> CauseID {
        self.causes.insert(Cause::Derived(DerivedCause {
            rule: StepRule::Derivation(DerivationStep { derivation_rule, parent_cause_id }),
        }))
    }

    pub fn apply_subst_with_causes(
        &mut self,
        pending_constraint: &PendingConstraint,
        engine: &TrackedEngine,
    ) -> Option<PendingConstraint> {
        let constraint = pending_constraint.constraint().apply_subst(&self.subst, engine)?;

        let mut binding_cause_ids = pending_constraint
            .interned_recursive_iter()
            .filter_map(|x| x.as_inference())
            .filter(|x| self.subst.has_inference_variable(x))
            .map(|x| self.subst_causes.get(x).copied().unwrap())
            .collect::<Vec<_>>();

        // remove duplicates
        binding_cause_ids.sort();
        binding_cause_ids.dedup();

        let cause_id =
            self.insert_derived_cause(StepRule::AppliedSubstitution(AppliedSubstitutionStep {
                original_cause_id: pending_constraint.cause_id(),
                binding_cause_ids,
            }));

        Some(PendingConstraint::builder().constraint(constraint).cause_id(cause_id).build())
    }

    fn insert_derived_cause(&mut self, rule: StepRule) -> CauseID {
        self.causes.insert(Cause::Derived(DerivedCause { rule }))
    }

    pub fn compose_subst(&mut self, subst: &Subst, binding_cause: CauseID, engine: &TrackedEngine) {
        for (existing_inference, existing_codo) in self.subst.inference_mappings() {
            // if the codomain of the existing inference will be mapped by the new
            // substitution, then the cause of the existing inference must be updated to
            // reflect that it is now derived from both the existing cause and the new
            // binding cause.
            if existing_codo
                .recursive_iter()
                .filter_map(|x| x.as_inference())
                .any(|x| subst.has_inference_variable(x))
            {
                let existing_cause_id =
                    self.subst_causes.get(&existing_inference).copied().unwrap();

                let new_cause_id = self.causes.insert(Cause::Derived(DerivedCause {
                    rule: StepRule::AppliedSubstitution(AppliedSubstitutionStep {
                        original_cause_id: existing_cause_id,
                        binding_cause_ids: vec![binding_cause],
                    }),
                }));

                self.subst_causes.insert(existing_inference, new_cause_id);
            }
        }

        // register new inferences in the substitution with the cause of the binding
        // that created them
        for (new_inference, _) in subst.inference_mappings() {
            self.subst_causes.entry(new_inference).or_insert(binding_cause);
        }

        // compose the substitutions, as usual
        self.subst.compose(subst, engine);
    }

    pub(super) fn primary_root_cause_id(&self, mut cause_id: CauseID) -> CauseID {
        loop {
            match &self.causes[cause_id] {
                Cause::Root(_root) => return cause_id,
                Cause::Derived(derived_cause) => match &derived_cause.rule {
                    StepRule::Derivation(step) => cause_id = step.parent_cause_id,
                    StepRule::AppliedSubstitution(step) => {
                        cause_id = step.original_cause_id;
                    }
                },
            }
        }
    }

    /// Traverses the cause graph to collect all the root causes that (including
    /// binding causes)
    fn collect_root_cause_ids(&self, cause_id: CauseID, roots: &mut FxHashSet<CauseID>) {
        match &self.causes[cause_id] {
            Cause::Root(_root) => {
                roots.insert(cause_id);
            }
            Cause::Derived(derived_cause) => match &derived_cause.rule {
                StepRule::Derivation(step) => {
                    self.collect_root_cause_ids(step.parent_cause_id, roots);
                }
                StepRule::AppliedSubstitution(step) => {
                    self.collect_root_cause_ids(step.original_cause_id, roots);
                    for binding_cause_id in &step.binding_cause_ids {
                        self.collect_root_cause_ids(*binding_cause_id, roots);
                    }
                }
            },
        }
    }

    fn resolved_effect_introduction_sites(
        &self,
        root_ids: &FxHashSet<CauseID>,
        engine: &TrackedEngine,
    ) -> Vec<(RelativeSpan, Interned<Ty>)> {
        let mut sites = Vec::new();

        for root_id in root_ids {
            let Cause::Root(root) = &self.causes[*root_id] else {
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
            sites.push((origin.span, introduced_effect));
        }

        sites.sort_by_key(|(span, _)| *span);
        sites.dedup();
        sites
    }

    fn first_distinct_effect_pair(
        sites: &[(RelativeSpan, Interned<Ty>)],
    ) -> Option<(RelativeSpan, Interned<Ty>, RelativeSpan, Interned<Ty>)> {
        for (index, (first_span, first_effect)) in sites.iter().enumerate() {
            for (second_span, second_effect) in &sites[index + 1..] {
                if first_effect != second_effect {
                    return Some((
                        *first_span,
                        first_effect.clone(),
                        *second_span,
                        second_effect.clone(),
                    ));
                }
            }
        }

        None
    }

    fn has_effect_composition_root(&self, root_ids: &FxHashSet<CauseID>) -> bool {
        root_ids.iter().any(|root_id| {
            let Cause::Root(root) = &self.causes[*root_id] else {
                return false;
            };

            match &root.origin {
                RootCauseOrigin::EffectSharing => true,

                RootCauseOrigin::Subtype(_) | RootCauseOrigin::EffectIntroduction(_) => false,
            }
        })
    }

    pub(super) fn effect_conflict(
        &self,
        cause_id: CauseID,
        engine: &TrackedEngine,
    ) -> Option<EffectConflict> {
        let mut root_ids = FxHashSet::default();
        // collect all the root causes that contributed to the failure of this cause.
        self.collect_root_cause_ids(cause_id, &mut root_ids);

        if !self.has_effect_composition_root(&root_ids) {
            return None;
        }

        // collect all the effect introduction sites that contributed to the failure of
        // this cause.
        let sites = self.resolved_effect_introduction_sites(&root_ids, engine);

        let (first_span, first_effect, second_span, second_effect) =
            Self::first_distinct_effect_pair(&sites)?;

        Some(EffectConflict { first_span, first_effect, second_span, second_effect })
    }

    pub(super) fn resolved_subtype_origin(
        &self,
        root_cause_id: CauseID,
        engine: &TrackedEngine,
    ) -> Option<(SubtypeSource, RelativeSpan, Subtype)> {
        let Cause::Root(root) = &self.causes[root_cause_id] else {
            return None;
        };
        let RootCauseOrigin::Subtype(origin) = &root.origin else {
            return None;
        };

        Some((
            origin.source,
            origin.span,
            origin.original_subtype.apply_subst_or_clone(&self.subst, engine),
        ))
    }

    pub(super) fn default_unbound_inferences(
        &mut self,
        inferences: impl IntoIterator<Item = Inference>,
        default: &Interned<Ty>,
        engine: &TrackedEngine,
    ) {
        let defaults = inferences
            .into_iter()
            .filter(|inference| self.subst.get(inference).is_none())
            .map(|inference| (inference, default.clone()))
            .collect();
        self.subst.compose(&defaults, engine);
    }

    pub(super) fn into_subst(self) -> Subst { self.subst }
}

impl TAstBuilder {
    pub fn latest_type(&self, ty: &Interned<Ty>) -> Interned<Ty> {
        ty.apply_subst_or_clone(&self.constraint_solver.provenance.subst, &self.engine)
    }
}
