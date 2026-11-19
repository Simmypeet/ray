//! Explanations of unprovable goals, joining why the solver failed with why
//! the site required the goal so reporting is decided in one place.

mod failure;
mod predicate;
mod render;

pub use failure::{
    BuiltInMarkerRule, CandidateFailure, CoroutineSavedLocal, DropComponent, Goal, Leaf,
    ProofFailure, Reason, Step, StructuralComponent,
};
pub use render::{ExplainedGoal, RequiredBy, UnexplainedPredicate, UnsatisfiedPredicate};
