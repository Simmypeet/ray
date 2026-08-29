use bon::Builder;
use derive_more::From;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    constraint::{DerivationRule, subtype::Subtype},
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Builder)]
pub struct EffectSharingConstraintOrigin {
    child_expr_id: TypedFunctionLocalID<TypedExprID>,
    parent_expr_id: TypedFunctionLocalID<TypedExprID>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, From)]
pub enum RootCauseOrigin {
    Subtype(SubtypeConstraintOrigin),
    EffectSharing(EffectSharingConstraintOrigin),
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
            .constraint()
            .interned_recursive_iter()
            .filter_map(|x| x.as_inference())
            .filter_map(|x| {
                self.subst
                    .has_inference_variable(x)
                    .then(|| self.subst_causes.get(x).copied().unwrap())
            })
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

    pub fn apply_subst_with_causes_or_original(
        &mut self,
        pending_constraint: PendingConstraint,
        engine: &TrackedEngine,
    ) -> PendingConstraint {
        self.apply_subst_with_causes(&pending_constraint, engine).unwrap_or(pending_constraint)
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
}

impl TAstBuilder {
    pub fn latest_type(&self, ty: &Interned<Ty>) -> Interned<Ty> {
        ty.apply_subst_or_clone(&self.constraint_solver.provenance.subst, &self.engine)
    }
}
