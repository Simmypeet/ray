//! Implicit-instance resolution.
//!
//! The solver orchestrates lexical lookup, global candidate collection,
//! recursive premise solving, and final specificity ranking. Search state is
//! shared across every root resolved by one [`Solver`](crate::Solver).

use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_semantic_element::drop_plan::{DropPlan, GeneratedDropPlan, get_drop_plan};
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

impl InstanceCandidateFailure {
    #[must_use]
    pub(crate) fn as_limit_error(&self) -> Option<&InstanceResolutionError> {
        match self {
            Self::UndeterminedParameter(_) => None,
            Self::UnsatisfiedGiven { error, .. } => match &**error {
                InstanceResolutionError::Limit { .. } => Some(&**error),

                InstanceResolutionError::NotReady(_)
                | InstanceResolutionError::Cycle(_)
                | InstanceResolutionError::ContainsError(_)
                | InstanceResolutionError::AmbiguousLexical { .. }
                | InstanceResolutionError::NoInstance { .. }
                | InstanceResolutionError::AmbiguousGlobal { .. } => None,
            },
        }
    }

    #[must_use]
    pub(crate) fn as_cycle_error(&self) -> Option<&InstanceResolutionCycle> {
        match self {
            Self::UndeterminedParameter(_) => None,
            Self::UnsatisfiedGiven { error, .. } => match &**error {
                InstanceResolutionError::Cycle(cycle) => Some(cycle),
                InstanceResolutionError::NotReady(_)
                | InstanceResolutionError::ContainsError(_)
                | InstanceResolutionError::AmbiguousLexical { .. }
                | InstanceResolutionError::NoInstance { .. }
                | InstanceResolutionError::AmbiguousGlobal { .. }
                | InstanceResolutionError::Limit { .. } => None,
            },
        }
    }
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
    /// Resolves a trait requirement to a lexical, built-in, or global
    /// dictionary term and the instantiated predicates required by its proof
    /// tree.
    ///
    /// Requirements must be ground, with lexical dictionaries taking
    /// precedence. Compiler-provided dictionaries are selected next, before
    /// matching global candidates. Global premises are resolved
    /// recursively, after which the unique most-specific viable head wins.
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

    /// Makes `NoDrop` discard its field's dictionary and returns a no-op Drop
    /// dictionary for the wrapper itself.
    pub(crate) async fn resolve_no_drop_instance(
        &self,
        required: &TraitRef,
    ) -> Option<ResolvedInstance> {
        // Accept only a Drop requirement with its single implementor argument.
        if required.args().len() != 1
            || required.trait_id() != self.engine().get_core_item(CoreItem::DropTrait).await
        {
            return None;
        }

        // Match the core wrapper by declaration identity before making its
        // intrinsic dictionary.
        let ty = required.args().interned_iter().next()?;
        let struct_ = ty.as_struct_view()?;
        if struct_.symbol_id() != self.engine().get_core_item(CoreItem::NoDropStruct).await {
            return None;
        }

        Some(ResolvedInstance::new(
            Ty::new_no_op_drop_instance(ty.clone(), self.engine()),
            Vec::new(),
        ))
    }

    /// Builds the intrinsic `Drop` dictionary for a tuple by resolving one
    /// dictionary for each element.
    pub(crate) async fn resolve_tuple_drop_instance(
        &mut self,
        required: &TraitRef,
    ) -> Option<InstanceResolutionResult> {
        let drop_trait = self.engine().get_core_item(CoreItem::DropTrait).await;
        if required.args().len() != 1 || required.trait_id() != drop_trait {
            return None;
        }

        // `required` is not owned by the solver, so its element types can be
        // borrowed while resolving them.
        let tuple = required.args().interned_iter().next()?;
        let elements = tuple.as_tuple_view()?.args();

        let resolution = self.resolve_element_drop_instances(elements, drop_trait).await;
        Some(resolution.map(|(element_instances, obligations)| {
            let term = Ty::new_tuple_drop_instance(tuple.clone(), element_instances, self.engine());
            ResolvedInstance::new(term, obligations)
        }))
    }

    /// Builds the intrinsic `Drop` dictionary for a closure by resolving one
    /// dictionary for each capture, exactly like the tuple of its captures.
    pub(crate) async fn resolve_closure_drop_instance(
        &mut self,
        required: &TraitRef,
    ) -> Option<InstanceResolutionResult> {
        let drop_trait = self.engine().get_core_item(CoreItem::DropTrait).await;
        if required.args().len() != 1 || required.trait_id() != drop_trait {
            return None;
        }

        let closure = required.args().interned_iter().next()?;
        let captured_tuple = closure.as_closure_view()?.captured_tuple();

        // Only the captures determine the dictionary, so readiness is checked
        // on them alone. The signature and effect row may stay uninferred,
        // e.g. for a closure that is never called.
        if captured_tuple.contains_inference() {
            return Some(Err(InstanceResolutionError::NotReady(required.clone())));
        }
        if captured_tuple.contains_error() {
            return Some(Err(InstanceResolutionError::ContainsError(required.clone())));
        }

        // Borrowed captures are pointers in the captured tuple, so their
        // dictionaries are no-ops and only by-value captures are dropped.
        let captures = captured_tuple.as_tuple_view()?.args();

        let resolution = self.resolve_element_drop_instances(captures, drop_trait).await;
        Some(resolution.map(|(capture_instances, obligations)| {
            let term =
                Ty::new_closure_drop_instance(closure.clone(), capture_instances, self.engine());
            ResolvedInstance::new(term, obligations)
        }))
    }

    /// Resolves one `Drop` dictionary per element, in element order, and
    /// collects the obligations of every selected proof tree.
    async fn resolve_element_drop_instances(
        &mut self,
        elements: &[Interned<Ty>],
        drop_trait: GlobalSymbolID,
    ) -> Result<(Vec<Interned<Ty>>, Vec<InstanceResolutionObligation>), InstanceResolutionError>
    {
        let mut element_instances = Vec::with_capacity(elements.len());
        let mut obligations = Vec::new();

        // Retain the selected element dictionaries, including lexical givens,
        // and propagate every obligation from their proof trees.
        Box::pin(async move {
            for element in elements {
                let element_requirement =
                    TraitRef::new(drop_trait, Args::new([element.clone()], self.engine()));
                let resolved = self.resolve_instance_from(element_requirement, None).await?;

                let (element_instance, element_obligations) = resolved.into_parts();
                element_instances.push(element_instance);
                extend_unique_obligations(&mut obligations, element_obligations);
            }

            Ok((element_instances, obligations))
        })
        .await
    }

    /// Selects only the dictionary prescribed by a nominal Drop plan.
    pub(crate) async fn resolve_nominal_drop_instance(
        &mut self,
        required: &TraitRef,
    ) -> Option<InstanceResolutionResult> {
        let drop_trait = self.engine().get_core_item(CoreItem::DropTrait).await;

        //  not a drop trait requirement
        if required.args().len() != 1 || required.trait_id() != drop_trait {
            return None;
        }

        let nominal = required.args().interned_iter().next()?.clone();
        let struct_ = nominal.as_struct_view()?;

        let plan = self.engine().get_drop_plan(struct_.symbol_id()).await;

        match &*plan {
            DropPlan::Explicit(instance_id) => {
                Some(self.resolve_planned_explicit_drop(required, *instance_id).await)
            }
            DropPlan::CannotDerive(_) => Some(Err(InstanceResolutionError::NoInstance {
                required: required.clone(),
                failed_candidates: Vec::new(),
            })),
            DropPlan::Generated(generated) => {
                Some(self.resolve_planned_generated_drop(nominal, generated, drop_trait).await)
            }
        }
    }

    async fn resolve_planned_generated_drop(
        &mut self,
        nominal: Interned<Ty>,
        generated: &GeneratedDropPlan,
        drop_trait: GlobalSymbolID,
    ) -> InstanceResolutionResult {
        let struct_ = nominal.as_struct_view().expect("a nominal type must be a struct");

        // Substitute the requested nominal arguments into the plan's generic
        // requirements before recursively resolving their Drop dictionaries.
        let substitution = struct_.create_subst(self.engine()).await;

        // this is due to the borrow-checker error
        let engine = self.engine().clone();

        Box::pin(async move {
            let mut external_instances = Vec::with_capacity(generated.requirements().len());
            let mut obligations = Vec::new();

            for ty in generated
                .requirements()
                .iter()
                .map(|ty| ty.apply_subst_or_clone(&substitution, &engine))
            {
                let requirement = TraitRef::new(drop_trait, Args::new([ty], &engine));
                let resolved = self.resolve_instance_from(requirement, None).await?;
                let (instance, nested_obligations) = resolved.into_parts();

                external_instances.push(instance);

                extend_unique_obligations(&mut obligations, nested_obligations);
            }

            let term = Ty::new_nominal_drop_instance(nominal, external_instances, self.engine());
            Ok(ResolvedInstance::new(term, obligations))
        })
        .await
    }

    async fn resolve_planned_explicit_drop(
        &mut self,
        required: &TraitRef,
        instance_id: GlobalSymbolID,
    ) -> InstanceResolutionResult {
        let Some(candidate) = candidates::selected(self, required, instance_id).await else {
            return Err(InstanceResolutionError::NoInstance {
                required: required.clone(),
                failed_candidates: Vec::new(),
            });
        };

        // Reuse normal premise and where-clause instantiation, but never
        // collect or rank any other global instance for this nominal type.
        match self.resolve_candidate(candidate).await {
            Ok(resolved) => Ok(resolved),
            Err(failure) => {
                if let Some(err) = failure.as_limit_error() {
                    return Err(err.clone());
                }
                if let Some(cycle) = failure.as_cycle_error() {
                    return Err(InstanceResolutionError::Cycle(cycle.clone()));
                }

                Err(InstanceResolutionError::NoInstance {
                    required: required.clone(),
                    failed_candidates: vec![FailedInstanceCandidate::new(instance_id, failure)],
                })
            }
        }
    }

    pub async fn search_active_goal(&mut self, required: &TraitRef) -> InstanceResolutionResult {
        // This wrapper's contract discards all Drop implementations, including
        // dictionaries that would otherwise be found lexically.
        if let Some(resolved) = self.resolve_no_drop_instance(required).await {
            return Ok(resolved);
        }

        // Drop for a type with a known constructor is decided by the compiler:
        // `NoDrop` (above), primitives and pointers are no-ops, tuples and
        // nominal types follow their structure or plan. Closures are resolved
        // earlier, before the readiness check. These run before
        // lexical lookup so a `given Drop[int32]` cannot replace the built-in
        // behavior; lexical Drop evidence is only consulted for opaque types
        // such as type variables and unreduced associated types.
        if let Some(resolved) = self.resolve_no_op_drop_instance(required).await {
            return Ok(resolved);
        }
        if let Some(resolution) = self.resolve_tuple_drop_instance(required).await {
            return resolution;
        }
        if let Some(resolution) = self.resolve_nominal_drop_instance(required).await {
            return resolution;
        }

        // Lexical evidence is the nearest dictionary for ordinary requirements.
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
                Ok(resolved) => viable.push(ViableInstance::new(instance_id, resolved)),
                Err(failure) => {
                    if let Some(err) = failure.as_limit_error() {
                        return Err(err.clone());
                    }

                    failures.push(FailedInstanceCandidate::new(instance_id, failure));
                }
            }
        }

        // has no viable candidates at all
        if viable.is_empty() {
            // however, if any candidate failed due to a cycle, report that instead
            // of the NoInstance error, since it is more informative than just saying
            // "no instance found"
            //
            // TODO: should we just return `NoInstance` with all the information
            // about failed candidates
            if let Some(cycle) =
                failures.iter().find_map(|candidate| candidate.failure().as_cycle_error())
            {
                return Err(InstanceResolutionError::Cycle(cycle.clone()));
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
    ) -> Result<ResolvedInstance, InstanceCandidateFailure> {
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
        Ok(ResolvedInstance::new(term, obligations))
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

#[cfg(test)]
mod test;
