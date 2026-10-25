//! Conversion of instance resolution errors into the failure tree.

use rayc_type::trait_ref::TraitRef;

use super::{
    FailedInstanceCandidate, InstanceCandidateFailure, InstanceResolutionError, NoInstanceReason,
};
use crate::explain::{CandidateFailure, Goal, Leaf, ProofFailure, Reason, Step};

impl ProofFailure {
    /// Explains why resolving `required` failed with `error`.
    ///
    /// Returns `None` when the failure involves the error type. Such a
    /// failure is never reported: the error type only exists because another
    /// diagnostic was already reported where it came from.
    #[must_use]
    pub fn from_instance_error(
        required: TraitRef,
        error: &InstanceResolutionError,
    ) -> Option<Self> {
        Some(Self::new(Goal::Instance(required), Reason::from_instance_error(error)?))
    }

    /// Explains the error of a sub-goal entered through `step`, as the
    /// reason its parent failed.
    fn requires(step: Step, error: &InstanceResolutionError) -> Option<Reason> {
        // A cycle or a limit names no requirement: it is about the search
        // path, which it carries with the step into each goal on it.
        let Some(required) = error.required() else {
            return Reason::from_instance_error(error);
        };
        let failure = Self::from_instance_error(required.clone(), error)?;

        // The path of a cycle already shows each goal on it and the step
        // into it, so a goal inside the cycle fails with the cycle itself
        // rather than through its step. The goal the cycle starts at keeps
        // the step that led to it.
        if let Some(cycle) = failure.reason().cycle()
            && !cycle.starts_at(required)
        {
            return Some(Reason::Leaf(Leaf::Cycle(cycle.clone())));
        }

        Some(Reason::Requires { step, failure: Box::new(failure) })
    }
}

impl Reason {
    /// Explains `error` as the failure of the requirement it names.
    fn from_instance_error(error: &InstanceResolutionError) -> Option<Self> {
        let leaf = match error {
            InstanceResolutionError::NotReady(_) => Leaf::NotReady,
            InstanceResolutionError::ContainsError(_) => return None,
            InstanceResolutionError::AmbiguousLexical { candidates, .. } => {
                Leaf::AmbiguousLexical(candidates.clone())
            }
            InstanceResolutionError::AmbiguousGlobal { candidates, .. } => {
                Leaf::AmbiguousGlobal(candidates.clone())
            }
            InstanceResolutionError::Cycle(cycle) => Leaf::Cycle(cycle.clone()),
            InstanceResolutionError::Limit { limit, recent_goals } => {
                Leaf::Limit { limit: *limit, recent_goals: recent_goals.clone() }
            }
            InstanceResolutionError::NoInstance { reason, .. } => match reason {
                NoInstanceReason::Candidates(candidates) => {
                    return Self::from_failed_candidates(candidates);
                }
                NoInstanceReason::Linear => Leaf::LinearType,
                NoInstanceReason::DropPlan(error) => Leaf::DropPlan(error.clone()),
            },
            InstanceResolutionError::BuiltinComponent { part, .. } => {
                return ProofFailure::requires(part.step, &part.error);
            }
        };

        Some(Self::Leaf(leaf))
    }

    /// Explains why every matching instance head failed.
    fn from_failed_candidates(candidates: &[FailedInstanceCandidate]) -> Option<Self> {
        match candidates {
            [] => Some(Self::Leaf(Leaf::NoMatchingInstance)),

            // With one candidate, its failure is the failure of the goal.
            [candidate] => Self::from_candidate_failure(candidate),

            candidates => candidates
                .iter()
                .map(|candidate| {
                    Self::from_candidate_failure(candidate)
                        .map(|reason| CandidateFailure::new(candidate.instance_id(), reason))
                })
                .collect::<Option<Vec<_>>>()
                .map(Self::CandidatesFailed),
        }
    }

    /// Explains why one matching instance head could not be completed.
    fn from_candidate_failure(candidate: &FailedInstanceCandidate) -> Option<Self> {
        match &candidate.failure {
            InstanceCandidateFailure::UndeterminedParameter(parameter) => {
                Some(Self::Leaf(Leaf::UndeterminedParameter(*parameter)))
            }
            InstanceCandidateFailure::UnsatisfiedGiven { parameter, error } => {
                let step = Step::NestedInstanceGiven {
                    instance_id: candidate.instance_id(),
                    parameter: *parameter,
                };
                ProofFailure::requires(step, error)
            }
        }
    }
}
