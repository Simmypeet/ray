//! Explanations of goals the solver could not prove.
//!
//! The solver explains why a goal failed, as a [`ProofFailure`]. The site
//! that asked for the goal explains why it was required, as a
//! [`RequiredBy`]. [`ExplainedGoal`] joins the two and is the one place
//! that decides how such a failure is reported. A where-clause predicate the
//! solver cannot explain is reported as an [`UnexplainedPredicate`] instead,
//! and [`UnsatisfiedPredicate`] holds either.

mod failure;
mod predicate;
mod render;

pub use failure::{
    BuiltInMarkerRule, CandidateFailure, CoroutineSavedLocal, DropComponent, Goal, Leaf,
    ProofFailure, Reason, Step, StructuralComponent,
};
pub use render::{ExplainedGoal, RequiredBy, UnexplainedPredicate, UnsatisfiedPredicate};
