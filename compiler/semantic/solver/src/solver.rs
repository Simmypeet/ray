use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::ty_relate::TyRelate,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{InferenceConstraint, Ty, TyKind, inference::Inference},
    where_clause::PredicateKind,
};

use crate::{
    givens::get_givens,
    solver::{
        instance_resolution_state::{InstanceResolutionLimits, InstanceResolutionState},
        marker_entailment::MarkerEntailmentState,
    },
    ty_relate::Step,
};

mod instance_resolution_state;
mod marker_entailment;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TyRelatingEnvironment {
    /// Normal type relation, where inference variables on either side may be
    /// bound to types of the same kind.
    Normal,

    /// One-way matching where only polymorphic variables on the lesser side
    /// may be bound. Used for matching instance heads.
    TopLevelMatching,
}

#[derive(Debug)]
pub struct Solver {
    inference_counter: u64,
    engine: TrackedEngine,
    site: GlobalSymbolID,
    instance_resolution: InstanceResolutionState,
    marker_entailment: MarkerEntailmentState,
    givens: Interned<[PredicateKind]>,
}

impl Solver {
    /// Returns whether two types are equal without binding any variables.
    ///
    /// This is intended for declaration checking and monomorphized types,
    /// where inference has already finished. A relation that can only be
    /// solved by producing a substitution is therefore not equality here.
    pub async fn eq_without_unify(&mut self, left: &Interned<Ty>, right: &Interned<Ty>) -> bool {
        let constraint = TyRelate::new(left.clone(), right.clone());
        let Some(substitution) =
            self.exhaustive_solve(vec![constraint], &TyRelatingEnvironment::Normal).await
        else {
            return false;
        };

        substitution.is_empty()
    }

    /// Returns whether two trait references have the same trait and equal
    /// arguments without binding any variables.
    pub async fn trait_refs_eq_without_unify(&mut self, left: &TraitRef, right: &TraitRef) -> bool {
        if left.trait_id() != right.trait_id() {
            return false;
        }
        let Some(arguments) = left.args().structural_match(right.args()) else {
            return false;
        };

        for (left, right) in arguments {
            if !self.eq_without_unify(left, right).await {
                return false;
            }
        }
        true
    }

    /// Returns whether `predicate` follows from this solver's visible givens
    /// without binding any variables.
    pub async fn entails_predicate(&mut self, predicate: &PredicateKind) -> bool {
        match predicate {
            PredicateKind::AssociatedTypeEquality(equality) => {
                self.eq_without_unify(equality.left(), equality.right()).await
            }
            PredicateKind::Marker(predicate) => {
                self.entails_marker_predicate(predicate.clone()).await
            }
        }
    }

    /// Matches an instance head against an expected trait reference, binding
    /// polymorphic variables on the head side using top-level matching.
    ///
    /// Returns `None` if the trait identities, argument counts, or types do not
    /// match.
    pub async fn head_match(&mut self, head: &TraitRef, expected: &TraitRef) -> Option<Subst> {
        if head.trait_id() != expected.trait_id() {
            return None;
        }

        let constrs = head
            .args()
            .structural_match(expected.args())?
            .map(|(head, expected)| TyRelate::new(head.clone(), expected.clone()))
            .collect();

        self.exhaustive_solve(constrs, &TyRelatingEnvironment::TopLevelMatching).await
    }

    /// Matches one type-constructor head against a concrete type without
    /// binding variables in the concrete type.
    pub(crate) async fn type_head_match(
        &mut self,
        head: Interned<Ty>,
        expected: Interned<Ty>,
    ) -> Option<Subst> {
        self.exhaustive_solve(
            vec![TyRelate::new(head, expected)],
            &TyRelatingEnvironment::TopLevelMatching,
        )
        .await
    }

    /// Solves all constraints, returning the composed substitution.
    ///
    /// Returns `None` if entailment fails or constraints remain after no
    /// further progress can be made.
    async fn exhaustive_solve(
        &mut self,
        mut constrs: Vec<TyRelate>,
        relate_env: &TyRelatingEnvironment,
    ) -> Option<Subst> {
        let mut subst = Subst::new_empty();
        let mut residual = Vec::<TyRelate>::new();

        while let Some(constraint) = constrs.pop() {
            match self.entail_ty_relate_with(&constraint, relate_env).await.ok()? {
                Step::Derived(derived) => {
                    constrs.extend(derived.into_iter().map(|derived| derived.ty_relate));
                }
                Step::Subst(new_subst) => {
                    subst.compose(&new_subst, &self.engine);
                    for constraint in &mut constrs {
                        constraint.apply_in_place(&new_subst, &self.engine);
                    }
                    residual.retain(|constraint| {
                        constraint.apply_subst(&new_subst, &self.engine).is_none_or(|updated| {
                            constrs.push(updated);
                            false
                        })
                    });
                }
                Step::NoProgress => {
                    if let Some(reduced) = constraint.reduce(&self.engine, self.givens()).await {
                        constrs.push(reduced);
                    } else {
                        residual.push(constraint);
                    }
                }
            }
        }

        residual.is_empty().then_some(subst)
    }

    /// Creates a solver without a declaration site or visible predicates.
    ///
    /// This is only appropriate after monomorphization, where every type is
    /// concrete, and in focused unit-test fixtures.
    #[must_use]
    pub fn without_givens(engine: TrackedEngine) -> Self {
        Self::with_givens(engine, GlobalSymbolID::default(), [])
    }

    /// Creates a solver with exactly the supplied visible predicates.
    ///
    /// Unlike [`Self::new`], this does not collect predicates from `site` and
    /// is suitable for entailment checks that must exclude the site's own
    /// where clause.
    pub fn with_givens(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        givens: impl IntoIterator<Item = PredicateKind>,
    ) -> Self {
        let givens = engine.intern_unsized(givens.into_iter().collect::<Vec<_>>());
        Self {
            inference_counter: 0,
            engine,
            site,
            instance_resolution: InstanceResolutionState::new(InstanceResolutionLimits::default()),
            marker_entailment: MarkerEntailmentState::default(),
            givens,
        }
    }

    /// Creates a solver with every predicate visible at `site`.
    pub async fn new(engine: TrackedEngine, site: GlobalSymbolID) -> Self {
        Self::with_limits(engine, site, InstanceResolutionLimits::default()).await
    }

    pub async fn with_limits(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        limits: InstanceResolutionLimits,
    ) -> Self {
        let givens = engine.get_givens(site).await;

        Self {
            inference_counter: 0,
            givens,
            engine,
            site,
            instance_resolution: InstanceResolutionState::new(limits),
            marker_entailment: MarkerEntailmentState::default(),
        }
    }

    /// Predicates visible at this solver's declaration site, nearest scope
    /// first.
    #[must_use]
    pub fn givens(&self) -> &[PredicateKind] { &self.givens }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    pub const fn site(&self) -> GlobalSymbolID { self.site }

    /// Reduces a value and its descendants until no further step is available.
    ///
    /// Reduction implementations must make progress toward termination.
    pub async fn normalize<T>(&self, value: &T) -> T
    where
        T: Reduce + Clone + PartialEq + Send,
    {
        let mut normalized = value.clone();
        while let Some(reduced) = normalized.reduce(self.engine(), self.givens()).await {
            assert!(reduced != normalized, "reduction must make progress");
            normalized = reduced;
        }
        normalized
    }

    #[must_use]
    pub const fn new_inference(&mut self, kind: TyKind) -> Inference {
        self.new_inference_with_constraint(kind, InferenceConstraint::Any)
    }

    #[must_use]
    pub const fn new_inference_with_constraint(
        &mut self,
        kind: TyKind,
        constraint: InferenceConstraint,
    ) -> Inference {
        let inference = Inference::new_with_constraint(kind, constraint, self.inference_counter);
        self.inference_counter += 1;
        inference
    }
}

#[cfg(test)]
mod test;
