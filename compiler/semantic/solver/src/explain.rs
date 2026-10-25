//! Explanations of goals the solver could not prove.
//!
//! The solver explains why a goal failed, as a [`ProofFailure`]. The site
//! that asked for the goal explains why it was required, as a
//! [`RequiredBy`]. [`UnsatisfiedGoal`] joins the two and is the one place
//! that decides how such a failure is reported.

mod failure;
mod predicate;
mod render;

pub use failure::{
    BuiltInMarkerRule, CandidateFailure, DropComponent, Goal, Leaf, ProofFailure, Reason, Step,
    StructuralComponent,
};
pub use render::{RequiredBy, UnsatisfiedGoal};
