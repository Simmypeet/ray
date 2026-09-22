//! Implicit-instance resolution.
//!
//! The solver orchestrates lexical lookup, global candidate collection,
//! recursive premise solving, and final specificity ranking. Search state is
//! shared across every root resolved by one [`Solver`](crate::Solver).

use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_type::{
    poly_var::{GlobalPolyVarID, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, args::Args},
    where_clause::{PredicateKind, get_where_clause},
};

mod candidates;
mod lexical;
mod ranking;

use candidates::InstanceCandidate;
use lexical::LexicalResolution;
use ranking::ViableInstance;

use crate::Solver;

/// Why one matching global candidate could not construct a dictionary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum InstanceCandidateFailure {
    UndeterminedParameter(GlobalPolyVarID),
    UnsatisfiedGiven { parameter: GlobalPolyVarID, error: Box<InstanceResolutionError> },
}

/// A matching global candidate whose complete prerequisite tree was not viable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum InstanceResolutionLimit {
    Depth { limit: usize },
    CandidateVisits { limit: usize, candidate: GlobalSymbolID },
}

/// One instantiated where-clause predicate required by a selected global
/// instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct InstanceResolutionObligation {
    instance_id: GlobalSymbolID,
    predicate: PredicateKind,
}

impl InstanceResolutionObligation {
    #[must_use]
    const fn new(instance_id: GlobalSymbolID, predicate: PredicateKind) -> Self {
        Self { instance_id, predicate }
    }

    #[must_use]
    pub fn into_parts(self) -> (GlobalSymbolID, PredicateKind) {
        (self.instance_id, self.predicate)
    }
}

/// A selected dictionary term and every predicate required by its resolution
/// tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct ResolvedInstance {
    term: Interned<Ty>,
    obligations: Vec<InstanceResolutionObligation>,
}

impl ResolvedInstance {
    #[must_use]
    const fn new(term: Interned<Ty>, obligations: Vec<InstanceResolutionObligation>) -> Self {
        Self { term, obligations }
    }

    #[must_use]
    pub fn into_parts(self) -> (Interned<Ty>, Vec<InstanceResolutionObligation>) {
        (self.term, self.obligations)
    }
}

/// A complete instance-resolution result eligible for memoization.
pub type InstanceResolutionResult = Result<ResolvedInstance, InstanceResolutionError>;

/// One canonical goal in a diagnostic search trace.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, qbice::StableHash, qbice::Encode, qbice::Decode,
)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
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
    /// Resolves a trait requirement to a built-in, lexical, or global
    /// dictionary term and the instantiated predicates required by its proof
    /// tree.
    ///
    /// Closure `Def` dictionaries resolve even with inference variables. Other
    /// requirements must be ground, with lexical dictionaries taking
    /// precedence. Otherwise all matching global candidates have their given
    /// premises resolved recursively, after which the unique most-specific
    /// viable head wins.
    pub async fn resolve_instance(&mut self, required: TraitRef) -> InstanceResolutionResult {
        self.resolve_instance_from(required, None).await
    }

    pub(crate) async fn resolve_closure_instance(
        &self,
        required: &TraitRef,
    ) -> Option<ResolvedInstance> {
        if required.args().len() != 1 {
            return None;
        }
        let closure = required.args().interned_iter().next()?;
        closure.as_closure_view()?;
        if required.trait_id() != self.engine().get_core_item(CoreItem::DefTrait).await {
            return None;
        }

        Some(ResolvedInstance::new(
            Ty::new_def_instance(closure.clone(), self.engine()),
            Vec::new(),
        ))
    }

    pub(crate) async fn resolve_no_op_drop_instance(
        &self,
        required: &TraitRef,
    ) -> Option<ResolvedInstance> {
        if required.args().len() != 1
            || required.trait_id() != self.engine().get_core_item(CoreItem::DropTrait).await
        {
            return None;
        }

        let ty = required.args().interned_iter().next()?;
        let Ty::Application(application) = &**ty else { return None };
        if !matches!(
            application.view(),
            rayc_type::ty::application::View::Primitive(_)
                | rayc_type::ty::application::View::Pointer(_)
        ) {
            return None;
        }

        Some(ResolvedInstance::new(
            Ty::new_no_op_drop_instance(ty.clone(), self.engine()),
            Vec::new(),
        ))
    }

    pub async fn search_active_goal(&mut self, required: &TraitRef) -> InstanceResolutionResult {
        match lexical::resolve(self, required).await {
            Ok(LexicalResolution::NotFound) => {}
            Ok(LexicalResolution::Resolved(term)) => {
                return Ok(ResolvedInstance::new(term, Vec::new()));
            }
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

        ranking::select(self, required, viable).await
    }

    async fn resolve_candidate(
        &mut self,
        candidate: InstanceCandidate,
    ) -> Result<ViableInstance, InstanceCandidateFailure> {
        let (mut subst, instance_id, pending_given_parameters) = candidate.into_parts();
        let parameters = self.engine().get_poly_var_map(instance_id).await;
        let mut obligations = Vec::new();

        Box::pin(async {
            for parameter_id in pending_given_parameters {
                let global_parameter_id = GlobalPolyVarID::new(instance_id, parameter_id);
                let required = parameters
                    .trait_ref_of(parameter_id)
                    .expect("a pending given parameter must have an instance requirement")
                    .apply_subst_or_clone(&subst, self.engine());

                let edge = InstanceResolutionEdge::new(instance_id, global_parameter_id);

                let resolved =
                    self.resolve_instance_from(required, Some(edge)).await.map_err(|error| {
                        InstanceCandidateFailure::UnsatisfiedGiven {
                            parameter: global_parameter_id,
                            error: Box::new(error),
                        }
                    })?;
                let (argument, nested_obligations) = resolved.into_parts();

                subst.compose(&Subst::new_singleton(global_parameter_id, argument), self.engine());
                extend_unique_obligations(&mut obligations, nested_obligations);
            }

            Ok(())
        })
        .await?;

        let mut arguments = Vec::with_capacity(parameters.len());
        for (parameter_id, _) in parameters.iter() {
            let global_parameter_id = GlobalPolyVarID::new(instance_id, parameter_id);
            let Some(argument) = subst.get(&global_parameter_id).cloned() else {
                return Err(InstanceCandidateFailure::UndeterminedParameter(global_parameter_id));
            };
            arguments.push(argument);
        }

        // Instantiate this candidate's predicates only after every ordinary and given
        // parameter has a selected argument.
        let where_clause = self.engine().get_where_clause(instance_id).await;
        extend_unique_obligations(
            &mut obligations,
            where_clause.iter().map(|predicate| {
                InstanceResolutionObligation::new(
                    instance_id,
                    predicate.kind().apply_subst_or_clone(&subst, self.engine()),
                )
            }),
        );

        let term =
            Ty::new_instance(instance_id, Args::new(arguments, self.engine()), self.engine());
        Ok(ViableInstance::new(instance_id, ResolvedInstance::new(term, obligations)))
    }
}

fn extend_unique_obligations(
    obligations: &mut Vec<InstanceResolutionObligation>,
    additional: impl IntoIterator<Item = InstanceResolutionObligation>,
) {
    for obligation in additional {
        if !obligations.contains(&obligation) {
            obligations.push(obligation);
        }
    }
}
