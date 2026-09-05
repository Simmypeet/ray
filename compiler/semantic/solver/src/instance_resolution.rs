//! Implicit-instance resolution.
//!
//! The solver orchestrates lexical lookup, global candidate collection,
//! recursive premise solving, and final specificity ranking. Search state is
//! shared across every root resolved by one [`Solver`](crate::Solver).

use qbice::storage::intern::Interned;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
};

mod candidates;
mod lexical;
mod ranking;

use candidates::InstanceCandidate;
use lexical::LexicalResolution;
use ranking::ViableInstance;

use crate::Solver;

/// Why one matching global candidate could not construct a dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceCandidateFailure {
    UndeterminedParameter(GlobalPolyVarID),
    UnsatisfiedGiven { parameter: GlobalPolyVarID, error: Box<InstanceResolutionError> },
}

/// A matching global candidate whose complete prerequisite tree was not viable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedInstanceCandidate {
    instance_id: GlobalSymbolID,
    failure: InstanceCandidateFailure,
}

impl FailedInstanceCandidate {
    #[must_use]
    pub(super) const fn new(
        instance_id: GlobalSymbolID,
        failure: InstanceCandidateFailure,
    ) -> Self {
        Self { instance_id, failure }
    }

    #[must_use]
    pub const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    #[must_use]
    pub const fn failure(&self) -> &InstanceCandidateFailure { &self.failure }
}

/// The candidate premise that caused a recursive goal to be entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceResolutionEdge {
    instance_id: GlobalSymbolID,
    given_parameter: GlobalPolyVarID,
}

impl InstanceResolutionEdge {
    #[must_use]
    pub const fn new(instance_id: GlobalSymbolID, given_parameter: GlobalPolyVarID) -> Self {
        Self { instance_id, given_parameter }
    }

    #[must_use]
    pub const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    #[must_use]
    pub const fn given_parameter(&self) -> GlobalPolyVarID { self.given_parameter }
}

/// The hard limit that stopped a search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceResolutionLimit {
    Depth { limit: usize },
    CandidateVisits { limit: usize, candidate: GlobalSymbolID },
}

/// A complete instance-resolution result eligible for memoization.
pub type InstanceResolutionResult = Result<Interned<Ty>, InstanceResolutionError>;

/// One canonical goal in a diagnostic search trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceResolutionFrame {
    goal: TraitRef,
    introduced_by: Option<InstanceResolutionEdge>,
}

impl InstanceResolutionFrame {
    #[must_use]
    pub const fn new(goal: TraitRef, introduced_by: Option<InstanceResolutionEdge>) -> Self {
        Self { goal, introduced_by }
    }

    #[must_use]
    pub const fn goal(&self) -> &TraitRef { &self.goal }

    #[must_use]
    pub const fn introduced_by(&self) -> Option<InstanceResolutionEdge> { self.introduced_by }
}

/// An exact cycle, including the repeated goal as the final frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceResolutionCycle {
    path: Vec<InstanceResolutionFrame>,
}

impl InstanceResolutionCycle {
    #[must_use]
    pub const fn new(path: Vec<InstanceResolutionFrame>) -> Self { Self { path } }

    #[must_use]
    pub fn path(&self) -> &[InstanceResolutionFrame] { &self.path }
}

/// A structured failure from implicit-instance resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceResolutionError {
    NotReady(TraitRef),
    ContainsError(TraitRef),
    AmbiguousLexical { required: TraitRef, candidates: Vec<GlobalPolyVarID> },
    NoInstance { required: TraitRef, failed_candidates: Vec<FailedInstanceCandidate> },
    AmbiguousGlobal { required: TraitRef, candidates: Vec<GlobalSymbolID> },
    Cycle(InstanceResolutionCycle),
    Limit { limit: InstanceResolutionLimit, recent_goals: Vec<InstanceResolutionFrame> },
}

impl Solver {
    /// Resolves a normalized, ground trait requirement to a lexical or global
    /// dictionary term.
    ///
    /// Lexical dictionaries form the first precedence tier. Otherwise all
    /// matching global candidates have their given premises resolved
    /// recursively, after which the unique most-specific viable head wins.
    pub async fn resolve_instance(
        &mut self,
        required: TraitRef,
    ) -> Result<Interned<Ty>, InstanceResolutionError> {
        self.resolve_instance_from(required, None).await
    }

    pub async fn search_active_goal(
        &mut self,
        required: &TraitRef,
    ) -> Result<Interned<Ty>, InstanceResolutionError> {
        match lexical::resolve(self.engine(), self.site(), required).await {
            Ok(LexicalResolution::NotFound) => {}
            Ok(LexicalResolution::Resolved(term)) => return Ok(term),
            Err(error) => return Err(error),
        }

        let candidates = match candidates::collect(self, required).await {
            Ok(candidates) => candidates,
            Err(error) => return Err(error),
        };
        let mut viable = Vec::new();
        let mut failures = Vec::new();

        for candidate in candidates {
            let instance_id = candidate.instance_id();
            match self.resolve_candidate(candidate).await {
                Ok(candidate) => viable.push(candidate),
                Err(failure) => {
                    if let InstanceCandidateFailure::UnsatisfiedGiven { error, .. } = &failure
                        && matches!(&**error, InstanceResolutionError::Limit {
                            limit: _,
                            recent_goals: _
                        })
                    {
                        return Err((**error).clone());
                    }
                    failures.push(FailedInstanceCandidate::new(instance_id, failure));
                }
            }
        }

        if viable.is_empty() {
            if let Some(cycle) = failures.iter().find_map(|candidate| match candidate.failure() {
                InstanceCandidateFailure::UnsatisfiedGiven { error, .. } => match &**error {
                    InstanceResolutionError::Cycle(cycle) => Some(cycle.clone()),
                    InstanceResolutionError::NotReady(_)
                    | InstanceResolutionError::ContainsError(_)
                    | InstanceResolutionError::AmbiguousLexical { .. }
                    | InstanceResolutionError::NoInstance { .. }
                    | InstanceResolutionError::AmbiguousGlobal { .. }
                    | InstanceResolutionError::Limit { .. } => None,
                },
                InstanceCandidateFailure::UndeterminedParameter(_) => None,
            }) {
                return Err(InstanceResolutionError::Cycle(cycle));
            }
            return Err(InstanceResolutionError::NoInstance {
                required: required.clone(),
                failed_candidates: failures,
            });
        }

        ranking::select(self, required, &viable).await
    }

    async fn resolve_candidate(
        &mut self,
        candidate: InstanceCandidate,
    ) -> Result<ViableInstance, InstanceCandidateFailure> {
        let (mut subst, instance_id, pending_given_parameters) = candidate.into_parts();
        let parameters = self.engine().get_poly_var_map(instance_id).await;

        for parameter_id in pending_given_parameters {
            let global_parameter_id = GlobalPolyVarID::new(instance_id, parameter_id);
            let required = parameters
                .trait_ref_of(parameter_id)
                .expect("a pending given parameter must have an instance requirement")
                .apply_subst_or_clone(&subst, self.engine());

            let edge = InstanceResolutionEdge::new(instance_id, global_parameter_id);

            let argument =
                self.resolve_instance_from(required, Some(edge)).await.map_err(|error| {
                    InstanceCandidateFailure::UnsatisfiedGiven {
                        parameter: global_parameter_id,
                        error: Box::new(error),
                    }
                })?;

            subst.compose(&Subst::new_singleton(global_parameter_id, argument), self.engine());
        }

        let mut arguments = Vec::with_capacity(parameters.len());
        for (parameter_id, _) in parameters.iter() {
            let global_parameter_id = GlobalPolyVarID::new(instance_id, parameter_id);
            let Some(argument) = subst.get(&global_parameter_id).cloned() else {
                return Err(InstanceCandidateFailure::UndeterminedParameter(global_parameter_id));
            };
            arguments.push(argument);
        }

        Ok(ViableInstance::new(
            instance_id,
            Ty::new_instance(instance_id, Args::new(arguments, self.engine()), self.engine()),
        ))
    }
}
