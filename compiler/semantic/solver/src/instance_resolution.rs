//! Implicit-instance resolution.
//!
//! The resolver orchestrates lexical lookup, global candidate collection,
//! recursive premise solving, and final specificity ranking. Search state is
//! shared across every root resolved by one `InstanceResolver`.

use std::{future::Future, pin::Pin};

use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    reduce::Reduce,
    solver::Solver,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
};

mod candidates;
mod lexical;
mod ranking;
mod state;

use candidates::InstanceCandidate;
use lexical::LexicalResolution;
use ranking::ViableInstance;
pub use state::{
    ActiveInstanceGoal, DEFAULT_MAX_CANDIDATE_VISITS, DEFAULT_MAX_DEPTH, EnteredInstanceGoal,
    InstanceResolutionCycle, InstanceResolutionEdge, InstanceResolutionFrame,
    InstanceResolutionLimit, InstanceResolutionLimits, InstanceResolutionResult,
    InstanceResolutionState, InstanceResolutionStateError,
};

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

impl From<InstanceResolutionStateError> for InstanceResolutionError {
    fn from(error: InstanceResolutionStateError) -> Self {
        match error {
            InstanceResolutionStateError::Cycle(cycle) => Self::Cycle(cycle),
            InstanceResolutionStateError::Limit { limit, recent_goals } => {
                Self::Limit { limit, recent_goals }
            }
        }
    }
}

/// Resolves instance requirements occurring at one semantic site.
#[derive(Debug)]
pub struct InstanceResolver {
    engine: TrackedEngine,
    site: GlobalSymbolID,
    state: InstanceResolutionState,
}

impl InstanceResolver {
    #[must_use]
    pub fn new(engine: TrackedEngine, site: GlobalSymbolID) -> Self {
        Self { engine, site, state: InstanceResolutionState::default() }
    }

    #[must_use]
    pub fn with_limits(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        limits: InstanceResolutionLimits,
    ) -> Self {
        Self { engine, site, state: InstanceResolutionState::new(limits) }
    }

    /// Resolves a normalized, ground trait requirement to a lexical or global
    /// dictionary term.
    ///
    /// Lexical dictionaries form the first precedence tier. Otherwise all
    /// matching global candidates have their given premises resolved
    /// recursively, after which the unique most-specific viable head wins.
    pub async fn resolve_instance(
        &mut self,
        solver: &mut Solver,
        required: TraitRef,
    ) -> Result<Interned<Ty>, InstanceResolutionError> {
        self.resolve_instance_from(solver, required, None).await
    }

    fn resolve_instance_from<'b>(
        &'b mut self,
        solver: &'b mut Solver,
        required: TraitRef,
        introduced_by: Option<InstanceResolutionEdge>,
    ) -> Pin<Box<dyn Future<Output = Result<Interned<Ty>, InstanceResolutionError>> + 'b>> {
        Box::pin(async move {
            let engine = self.engine.clone();
            let required = required.normalize(&engine);
            if required.contains_inference() {
                return Err(InstanceResolutionError::NotReady(required));
            }
            if required.contains_error() {
                return Err(InstanceResolutionError::ContainsError(required));
            }

            let active = match self.state.enter_goal(required.clone(), introduced_by)? {
                EnteredInstanceGoal::Memoized(result) => return result,
                EnteredInstanceGoal::Active(active) => active,
            };

            let result = self.search_active_goal(solver, &required).await;
            self.state.complete_goal(active, result.clone());
            result
        })
    }

    async fn search_active_goal(
        &mut self,
        solver: &mut Solver,
        required: &TraitRef,
    ) -> Result<Interned<Ty>, InstanceResolutionError> {
        let engine = self.engine.clone();
        match lexical::resolve(&engine, self.site, required).await {
            Ok(LexicalResolution::NotFound) => {}
            Ok(LexicalResolution::Resolved(term)) => return Ok(term),
            Err(error) => return Err(error),
        }

        let candidates = match candidates::collect(
            &engine,
            solver,
            &mut self.state,
            self.site,
            required,
        )
        .await
        {
            Ok(candidates) => candidates,
            Err(error) => return Err(error.into()),
        };
        let mut viable = Vec::new();
        let mut failures = Vec::new();

        for candidate in candidates {
            let instance_id = candidate.instance_id();
            match self.resolve_candidate(solver, candidate).await {
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
            return Err(InstanceResolutionError::NoInstance {
                required: required.clone(),
                failed_candidates: failures,
            });
        }

        ranking::select(solver, required, &viable).await
    }

    async fn resolve_candidate(
        &mut self,
        solver: &mut Solver,
        candidate: InstanceCandidate,
    ) -> Result<ViableInstance, InstanceCandidateFailure> {
        let (mut subst, instance_id, pending_given_parameters) = candidate.into_parts();
        let parameters = self.engine.get_poly_var_map(instance_id).await;

        for parameter_id in pending_given_parameters {
            let global_parameter_id = GlobalPolyVarID::new(instance_id, parameter_id);
            let required = parameters
                .trait_ref_of(parameter_id)
                .expect("a pending given parameter must have an instance requirement")
                .apply_subst_or_clone(&subst, &self.engine);

            let edge = InstanceResolutionEdge::new(instance_id, global_parameter_id);

            let argument = self.resolve_instance_from(solver, required, Some(edge)).await.map_err(
                |error| InstanceCandidateFailure::UnsatisfiedGiven {
                    parameter: global_parameter_id,
                    error: Box::new(error),
                },
            )?;

            subst.compose(&Subst::new_singleton(global_parameter_id, argument), &self.engine);
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
            Ty::new_instance(instance_id, Args::new(arguments, &self.engine), &self.engine),
        ))
    }
}

#[cfg(test)]
mod test;
