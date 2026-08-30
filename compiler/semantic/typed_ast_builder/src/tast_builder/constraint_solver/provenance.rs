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
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct SubtypeConstraintOrigin {
    original_subtype: Subtype,
    source: SubtypeSource,
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct EffectUnificationOrigin {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
    source: EffectUnificationSource,
    span: RelativeSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum EffectUnificationSource {
    EffectSharing,
    EffectIntroduction,
    FunctionBodyEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, From)]
pub enum RootCauseOrigin {
    Subtype(SubtypeConstraintOrigin),
    EffectUnification(EffectUnificationOrigin),
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
pub(super) struct ResolvedEffectUnification {
    pub(super) lesser: Interned<Ty>,
    pub(super) greater: Interned<Ty>,
    pub(super) source: EffectUnificationSource,
    pub(super) span: RelativeSpan,
    pub(super) related_sites: Vec<ResolvedEffectUnificationSite>,
    pub(super) root_ids: FxHashSet<CauseID>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResolvedEffectUnificationSite {
    pub(super) effect_row: Interned<Ty>,
    pub(super) source: EffectUnificationSource,
    pub(super) span: RelativeSpan,
}

pub(super) enum ResolvedRootCause {
    Subtype { source: SubtypeSource, span: RelativeSpan, subtype: Subtype },
    EffectUnification(ResolvedEffectUnification),
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

    pub(super) fn root_cause_ids(&self, cause_id: CauseID) -> FxHashSet<CauseID> {
        let mut root_ids = FxHashSet::default();
        self.collect_root_cause_ids(cause_id, &mut root_ids);
        root_ids
    }

    fn effect_unification_sites(
        &self,
        root_ids: &FxHashSet<CauseID>,
        primary_root_id: CauseID,
        primary_span: RelativeSpan,
        engine: &TrackedEngine,
    ) -> Vec<ResolvedEffectUnificationSite> {
        let mut sites = Vec::new();

        for root_id in root_ids {
            if *root_id == primary_root_id {
                continue;
            }

            let Cause::Root(root) = &self.causes[*root_id] else {
                continue;
            };
            let RootCauseOrigin::EffectUnification(origin) = &root.origin else {
                continue;
            };
            if origin.span == primary_span {
                continue;
            }

            match origin.source {
                EffectUnificationSource::EffectIntroduction => {
                    sites.push(ResolvedEffectUnificationSite {
                        effect_row: self.resolve_type(&origin.lesser, engine),
                        source: origin.source,
                        span: origin.span,
                    });
                }
                EffectUnificationSource::FunctionBodyEffect => {
                    sites.push(ResolvedEffectUnificationSite {
                        effect_row: self.resolve_type(&origin.greater, engine),
                        source: origin.source,
                        span: origin.span,
                    });
                }
                EffectUnificationSource::EffectSharing => {}
            }
        }

        sites.sort_by(|first, second| {
            (&first.span, &first.source, &first.effect_row).cmp(&(
                &second.span,
                &second.source,
                &second.effect_row,
            ))
        });
        sites.dedup();
        sites
    }

    fn resolve_type(&self, ty: &Interned<Ty>, engine: &TrackedEngine) -> Interned<Ty> {
        let mut ty = ty.apply_subst_or_clone(&self.subst, engine);
        while let Some(reduced) = ty.reduce(engine) {
            ty = reduced;
        }
        ty
    }

    pub(super) fn resolved_root_cause(
        &self,
        primary_root_id: CauseID,
        root_ids: FxHashSet<CauseID>,
        engine: &TrackedEngine,
    ) -> ResolvedRootCause {
        let Cause::Root(root) = &self.causes[primary_root_id] else {
            unreachable!("the primary root cause ID must identify a root cause")
        };

        match &root.origin {
            RootCauseOrigin::Subtype(origin) => ResolvedRootCause::Subtype {
                source: origin.source,
                span: origin.span,
                subtype: origin.original_subtype.apply_subst_or_clone(&self.subst, engine),
            },
            RootCauseOrigin::EffectUnification(origin) => {
                ResolvedRootCause::EffectUnification(ResolvedEffectUnification {
                    lesser: self.resolve_type(&origin.lesser, engine),
                    greater: self.resolve_type(&origin.greater, engine),
                    source: origin.source,
                    span: origin.span,
                    related_sites: self.effect_unification_sites(
                        &root_ids,
                        primary_root_id,
                        origin.span,
                        engine,
                    ),
                    root_ids,
                })
            }
        }
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
