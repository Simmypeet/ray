//! The failure tree: why a goal could not be proved.

use qbice::{Decode, Encode, StableHash};
use rayc_semantic_element::{drop_plan::DropPlanError, struct_body::FieldID};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{poly_var::GlobalPolyVarID, trait_ref::TraitRef, where_clause::MarkerPredicate};

use crate::instance_resolution::{
    InstanceResolutionCycle, InstanceResolutionFrame, InstanceResolutionLimit,
};

/// A goal the solver was asked to prove.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Goal {
    /// A dictionary of the trait reference.
    Instance(TraitRef),

    /// A type satisfies a marker.
    Marker(MarkerPredicate),
}

/// Why a goal could not be proved: the reason it failed, which may be a
/// sub-goal that failed in turn.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ProofFailure {
    goal: Goal,
    reason: Reason,
}

impl ProofFailure {
    #[must_use]
    pub const fn new(goal: Goal, reason: Reason) -> Self { Self { goal, reason } }

    #[must_use]
    pub const fn goal(&self) -> &Goal { &self.goal }

    #[must_use]
    pub const fn reason(&self) -> &Reason { &self.reason }

    /// Returns the failure of the innermost part of a compiler-built
    /// dictionary, or `self` when the goal is not one.
    ///
    /// A compiler-built dictionary fails because one of its parts does, so
    /// the innermost part is the goal the failure is about.
    #[must_use]
    pub fn innermost_part(&self) -> &Self {
        match &self.reason {
            Reason::Requires {
                step: Step::DropComponent(_) | Step::ContinuationCoroutine,
                failure,
            } => failure.innermost_part(),
            Reason::Requires {
                step:
                    Step::NestedInstanceGiven { .. }
                    | Step::MarkerImplementationPremise { .. }
                    | Step::Structural(_),
                ..
            }
            | Reason::CandidatesFailed(_)
            | Reason::Leaf(_) => self,
        }
    }
}

/// Why a goal failed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Reason {
    /// The goal failed because a sub-goal it requires failed.
    Requires { step: Step, failure: Box<ProofFailure> },

    /// Several instance heads matched, and none could be completed.
    CandidatesFailed(Vec<CandidateFailure>),

    /// The goal failed for a reason of its own.
    Leaf(Leaf),
}

impl Reason {
    /// Returns the cycle when the goal failed because proving it requires
    /// proving it again.
    #[must_use]
    pub const fn cycle(&self) -> Option<&InstanceResolutionCycle> {
        match self {
            Self::Leaf(Leaf::Cycle(cycle)) => Some(cycle),

            Self::Leaf(
                Leaf::NoMatchingInstance
                | Leaf::AmbiguousLexical(_)
                | Leaf::AmbiguousGlobal(_)
                | Leaf::UndeterminedParameter(_)
                | Leaf::LinearType
                | Leaf::DropPlan(_)
                | Leaf::Limit { .. }
                | Leaf::NotReady
                | Leaf::NegativeImplementation(_)
                | Leaf::BuiltIn(_)
                | Leaf::Opaque
                | Leaf::Unexplained,
            )
            | Self::Requires { .. }
            | Self::CandidatesFailed(_) => None,
        }
    }
}

/// One matching instance head that could not be completed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct CandidateFailure {
    instance_id: GlobalSymbolID,

    /// Either a given that failed, as [`Reason::Requires`] with
    /// [`Step::InstanceGiven`], or a [`Leaf::UndeterminedParameter`].
    reason: Reason,
}

impl CandidateFailure {
    #[must_use]
    pub const fn new(instance_id: GlobalSymbolID, reason: Reason) -> Self {
        Self { instance_id, reason }
    }

    #[must_use]
    pub const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    #[must_use]
    pub const fn reason(&self) -> &Reason { &self.reason }
}

/// Why a sub-goal was entered: the edge from a goal to the goal it requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Step {
    /// A given parameter of a candidate instance.
    NestedInstanceGiven { instance_id: GlobalSymbolID, parameter: GlobalPolyVarID },

    /// A part of a compiler-built `Drop` dictionary: the dictionary of a
    /// value that dropping the type drops.
    DropComponent(DropComponent),

    /// The `Coroutine` dictionary that the `Def` dictionary of a
    /// `core.Continuation` is built from.
    ContinuationCoroutine,

    /// A premise of the explicit marker implementation that applies to the
    /// type.
    MarkerImplementationPremise { implementation_id: GlobalSymbolID },

    /// A value stored by a type without an explicit marker implementation,
    /// which satisfies a marker only when everything it stores does.
    Structural(StructuralComponent),
}

/// A value stored inside a type, which is dropped with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum DropComponent {
    /// An element of a tuple, by index.
    TupleElement(usize),

    /// A by-value capture of a closure, by index into its captured tuple.
    ClosureCapture(usize),

    /// An argument of a suspended function, which its coroutine holds until
    /// it is first resumed, by parameter index.
    CoroutineUpvar(usize),

    /// A type that the generated `Drop` plan of a struct requires, by index
    /// into the plan's requirements.
    DropPlanRequirement(usize),
}

/// A value stored by a type, which decides the markers the type satisfies
/// when no explicit marker implementation applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum StructuralComponent {
    /// A field of a struct.
    StructField(FieldID),

    /// The payload of an enum variant, by its `SymbolKind::Variant` symbol.
    VariantPayload(GlobalSymbolID),

    /// An element of a tuple, by index.
    TupleElement(usize),

    /// The tuple of everything a closure captures, when the tuple has an
    /// explicit marker implementation of its own.
    ClosureCaptures,

    /// One capture of a closure, by index into its captured tuple.
    ClosureCapture(usize),
}

/// A marker goal that the compiler rejects whatever the givens say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum BuiltInMarkerRule {
    /// A linear struct or enum is never `Copy`.
    LinearTypeNotCopy,

    /// A coroutine is never `Copy`.
    CoroutineNotCopy,

    /// A unique reference is never `Copy`.
    UniqueReferenceNotCopy,
}

/// The reason a goal failed on its own, which the programmer acts on.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum Leaf {
    /// No instance head matches the goal.
    NoMatchingInstance,

    /// Several givens in scope provide the goal.
    AmbiguousLexical(Vec<GlobalPolyVarID>),

    /// Several global instances provide the goal, and none is more specific
    /// than the others.
    AmbiguousGlobal(Vec<GlobalSymbolID>),

    /// The only matching instance has a parameter that the goal does not
    /// determine.
    UndeterminedParameter(GlobalPolyVarID),

    /// The type is `@linear`, so it has no `Drop` dictionary.
    LinearType,

    /// The `Drop` plan of the struct cannot be generated.
    DropPlan(DropPlanError),

    /// Proving the goal requires proving it again.
    Cycle(InstanceResolutionCycle),

    /// The search stopped at a hard limit.
    Limit { limit: InstanceResolutionLimit, recent_goals: Vec<InstanceResolutionFrame> },

    /// The goal still has inference variables that decide which instance
    /// is selected.
    NotReady,

    /// A negative marker implementation excludes the type.
    NegativeImplementation(GlobalSymbolID),

    /// The compiler rejects the marker goal itself.
    BuiltIn(BuiltInMarkerRule),

    /// No marker rule applies to the type, and no given provides the goal:
    /// the type is a type variable, a reference or a pointer.
    Opaque,

    /// The marker goal fails, but its failing premise is not known: the
    /// search for it stopped at its budget, or the explicit implementation
    /// that applies has an equality premise, which is rejected where the
    /// implementation is declared.
    Unexplained,
}
