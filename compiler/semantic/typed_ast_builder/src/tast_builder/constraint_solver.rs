use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_hash::FxHashMap;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::{
    constraint::{self, Constraint, DerivationRule, subtype::Subtype},
    solver::Solver,
    subst::Subst,
    ty::{Ty, inference::Inference},
};
use rayc_typed_ast::{typed_expr::TypedExprID, typed_function::TypedFunctionLocalID};

mod constraints;
mod diagnostics;
mod provenance;
mod solve;

#[cfg(test)]
mod tests;

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubtypeConstraintOrigin {
    original_subtype: Subtype,
    source: SubtypeSource,
    span: RelativeSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectIntroductionConstraintOrigin {
    expression_id: TypedFunctionLocalID<TypedExprID>,
    span: RelativeSpan,
    introduced_effect: Interned<Ty>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EffectSharingConstraintOrigin {
    child_expr_id: TypedFunctionLocalID<TypedExprID>,
    parent_expr_id: TypedFunctionLocalID<TypedExprID>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RootCauseOrigin {
    Subtype(SubtypeConstraintOrigin),
    EffectSharing(EffectSharingConstraintOrigin),
    EffectIntroduction(EffectIntroductionConstraintOrigin),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RootCause {
    origin: RootCauseOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExplanationRule {
    ConstraintDerivation(DerivationRule),

    /// The first parent is the constraint that was rewritten. The remaining
    /// parents explain the inference bindings used to rewrite it.
    AppliedSubstitution,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DerivedCause {
    rule: ExplanationRule,

    // Parent order is significant: the first parent is the primary cause used
    // for ordinary diagnostics; later parents are contributing explanations.
    parent_causes: Vec<CauseID>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    Root(RootCause),
    Derived(DerivedCause),
}

pub type CauseID = ID<Cause>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PendingConstraint {
    constraint: Constraint,
    cause_id: CauseID,
}

#[derive(Debug)]
pub struct ConstraintSolver {
    causes: Arena<Cause>,

    residual_constraints: Vec<PendingConstraint>,
    errored_constraints: Vec<(constraint::Error, PendingConstraint)>,

    numeric_inferences: Vec<Inference>,

    subst: Subst,

    // `subst` stores what each inference is equal to; this parallel map stores
    // why that binding exists. For example:
    //
    //   subst:        ?parent -> {State[int32] | ?tail}
    //   subst_causes: ?parent -> the introduction/sharing path from `intCall()`
    //
    // Keeping the cause separately lets a later constraint rewritten through
    // `?parent` retain the source of `State[int32]` in its explanation.
    subst_causes: FxHashMap<Inference, CauseID>,
    solver: Solver,
}
