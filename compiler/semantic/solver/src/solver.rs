use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::{outlives::OutlivesConstraints, ty_relate::TyRelate},
    reduce::Reduce,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{InferenceConstraint, Ty, TyKind, inference::Inference},
    where_clause::PredicateKind,
};

use crate::{
    givens::get_givens,
    inference_generator::{CountingInferenceGenerator, InferenceGenerator},
    outlives::{OutlivesEnvironment, get_outlives_environment, outlives_givens},
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
    ///
    /// Relations in this environment must be [`Variance::Invariant`], since
    /// head arguments are invariant.
    ///
    /// [`Variance::Invariant`]: rayc_type::variance::Variance::Invariant
    TopLevelMatching,
}

/// The result of solving a set of relations: the substitution that solves
/// them, and the outlives constraints they require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solution {
    subst: Subst,
    outlives: OutlivesConstraints,
}

impl Solution {
    #[must_use]
    pub const fn subst(&self) -> &Subst { &self.subst }

    /// The outlives constraints the relations require, with the substitution
    /// applied.
    #[must_use]
    pub const fn outlives(&self) -> &OutlivesConstraints { &self.outlives }

    #[must_use]
    pub fn into_parts(self) -> (Subst, OutlivesConstraints) { (self.subst, self.outlives) }
}

#[derive(Debug)]
pub struct Solver {
    inference_generator: Box<dyn InferenceGenerator>,
    engine: TrackedEngine,
    site: GlobalSymbolID,
    instance_resolution: InstanceResolutionState,
    marker_entailment: MarkerEntailmentState,
    givens: Interned<[PredicateKind]>,

    /// The outlives facts visible at `site`; see [`OutlivesEnvironment`].
    outlives_environment: Interned<OutlivesEnvironment>,
}

impl Solver {
    /// Returns whether two types are equal without binding any variables.
    ///
    /// This is intended for declaration checking and monomorphized types,
    /// where inference has already finished. A relation that can only be
    /// solved by producing a substitution is therefore not equality here.
    /// Lifetimes never decide equality; see [`Self::relate_without_unify`].
    pub async fn eq_without_unify(&mut self, left: &Interned<Ty>, right: &Interned<Ty>) -> bool {
        self.relate_without_unify(left, right).await.is_some()
    }

    /// Returns the outlives constraints that make two types equal, if they
    /// are equal without binding any variables; see
    /// [`Self::eq_without_unify`].
    ///
    /// Lifetimes never decide whether two types are equal. Each pair of
    /// corresponding lifetimes is related invariantly instead.
    pub async fn relate_without_unify(
        &mut self,
        left: &Interned<Ty>,
        right: &Interned<Ty>,
    ) -> Option<OutlivesConstraints> {
        let constraint = TyRelate::new_invariant(left.clone(), right.clone());
        self.solve_without_unify(vec![constraint]).await
    }

    /// Returns whether two trait references have the same trait and equal
    /// arguments without binding any variables.
    pub async fn trait_refs_eq_without_unify(&mut self, left: &TraitRef, right: &TraitRef) -> bool {
        self.relate_trait_refs_without_unify(left, right).await.is_some()
    }

    /// Returns the outlives constraints that make two trait references equal,
    /// if they are equal without binding any variables; see
    /// [`Self::relate_without_unify`].
    pub async fn relate_trait_refs_without_unify(
        &mut self,
        left: &TraitRef,
        right: &TraitRef,
    ) -> Option<OutlivesConstraints> {
        if left.trait_id() != right.trait_id() {
            return None;
        }

        // Trait arguments are invariant.
        let constraints = left
            .args()
            .structural_match(right.args())?
            .map(|(left, right)| TyRelate::new_invariant(left.clone(), right.clone()))
            .collect();
        self.solve_without_unify(constraints).await
    }

    /// Solves relations that must hold without binding any variables, and
    /// returns the outlives constraints they require.
    async fn solve_without_unify(
        &mut self,
        constraints: Vec<TyRelate>,
    ) -> Option<OutlivesConstraints> {
        let solution = self.exhaustive_solve(constraints, &TyRelatingEnvironment::Normal).await?;
        let (subst, outlives) = solution.into_parts();
        subst.is_empty().then_some(outlives)
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
            PredicateKind::Outlives(predicate) => self.entails_outlives(predicate).await,
        }
    }

    /// Matches an instance head against an expected trait reference, binding
    /// polymorphic variables on the head side using top-level matching.
    ///
    /// Returns `None` if the trait identities, argument counts, or types do not
    /// match. Lifetimes never decide whether a head matches: trait arguments
    /// are invariant, so each pair of corresponding lifetimes produces
    /// outlives constraints in both directions instead.
    pub async fn type_head_match(
        &mut self,
        head: &TraitRef,
        expected: &TraitRef,
    ) -> Option<Solution> {
        if head.trait_id() != expected.trait_id() {
            return None;
        }

        let constrs = head
            .args()
            .structural_match(expected.args())?
            .map(|(head, expected)| TyRelate::new_invariant(head.clone(), expected.clone()))
            .collect();

        self.exhaustive_solve(constrs, &TyRelatingEnvironment::TopLevelMatching).await
    }

    /// Matches one type-constructor head against a concrete type without
    /// binding variables in the concrete type.
    ///
    /// Only marker implementations are matched this way. They never depend
    /// on lifetimes, since a valid head is one constructor applied to
    /// distinct variables, so the match needs no outlives constraints.
    pub(crate) async fn simple_head_match(
        &mut self,
        head: Interned<Ty>,
        expected: Interned<Ty>,
    ) -> Option<Subst> {
        self.exhaustive_solve(
            vec![TyRelate::new_invariant(head, expected)],
            &TyRelatingEnvironment::TopLevelMatching,
        )
        .await
        .map(|solution| solution.into_parts().0)
    }

    /// Solves all constraints, returning the composed substitution and the
    /// outlives constraints they require.
    ///
    /// Returns `None` if entailment fails or constraints remain after no
    /// further progress can be made.
    async fn exhaustive_solve(
        &mut self,
        mut constrs: Vec<TyRelate>,
        relate_env: &TyRelatingEnvironment,
    ) -> Option<Solution> {
        let mut subst = Subst::new_empty();
        let mut residual = Vec::<TyRelate>::new();
        let mut outlives = OutlivesConstraints::new();

        while let Some(constraint) = constrs.pop() {
            let (step, new_outlives) =
                self.entail_ty_relate_with(&constraint, relate_env).await.ok()?.into_parts();
            outlives = outlives.union(new_outlives);

            match step {
                Step::Derived(derived) => {
                    constrs.extend(derived.into_iter().map(|derived| derived.ty_relate));
                }
                Step::Subst(new_subst) => {
                    self.compose_solved_subst(&mut subst, &new_subst, &mut constrs, &mut residual);
                }
                Step::Generalized { subst: new_subst, derived } => {
                    self.compose_solved_subst(&mut subst, &new_subst, &mut constrs, &mut residual);
                    constrs.extend(derived.into_iter().map(|derived| derived.ty_relate));
                }
                // The relation was normalized, so it waits for a binding.
                Step::NoProgress => residual.push(constraint),
            }
        }

        if !residual.is_empty() {
            return None;
        }

        // A variable bound after a constraint was produced is still replaced
        // in it.
        let outlives = outlives.apply_subst_or_clone(&subst, &self.engine);
        Some(Solution { subst, outlives })
    }

    /// Composes a new substitution into the solved one and applies it to the
    /// pending constraints, moving residual constraints it changes back to
    /// the pending ones.
    fn compose_solved_subst(
        &self,
        subst: &mut Subst,
        new_subst: &Subst,
        constrs: &mut Vec<TyRelate>,
        residual: &mut Vec<TyRelate>,
    ) {
        subst.compose(new_subst, &self.engine);
        for constraint in constrs.iter_mut() {
            constraint.apply_in_place(new_subst, &self.engine);
        }
        residual.retain(|constraint| {
            constraint.apply_subst(new_subst, &self.engine).is_none_or(|updated| {
                constrs.push(updated);
                false
            })
        });
    }

    /// Creates a solver without a declaration site or visible predicates.
    ///
    /// This is only appropriate after monomorphization, where every type is
    /// concrete, and in focused unit-test fixtures.
    pub async fn without_givens(engine: TrackedEngine) -> Self {
        Self::with_givens(engine, GlobalSymbolID::default(), []).await
    }

    /// Creates a solver with exactly the supplied visible predicates.
    ///
    /// Unlike [`Self::new`], this does not collect predicates from `site` and
    /// is suitable for entailment checks that must exclude the site's own
    /// where clause. The outlives facts are likewise exactly the supplied
    /// outlives predicates; the site's implied bounds are not included.
    pub async fn with_givens(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        givens: impl IntoIterator<Item = PredicateKind>,
    ) -> Self {
        let givens = engine.intern_unsized(givens.into_iter().collect::<Vec<_>>());
        let outlives_environment = engine
            .intern(OutlivesEnvironment::new(outlives_givens(&givens).cloned(), &engine).await);
        Self {
            inference_generator: Box::new(CountingInferenceGenerator::default()),
            engine,
            site,
            instance_resolution: InstanceResolutionState::new(InstanceResolutionLimits::default()),
            marker_entailment: MarkerEntailmentState::default(),
            givens,
            outlives_environment,
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
        let outlives_environment = engine.get_outlives_environment(site).await;

        Self {
            inference_generator: Box::new(CountingInferenceGenerator::default()),
            givens,
            engine,
            site,
            instance_resolution: InstanceResolutionState::new(limits),
            marker_entailment: MarkerEntailmentState::default(),
            outlives_environment,
        }
    }

    /// Predicates visible at this solver's declaration site, nearest scope
    /// first.
    #[must_use]
    pub fn givens(&self) -> &[PredicateKind] { &self.givens }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    pub const fn site(&self) -> GlobalSymbolID { self.site }

    /// Returns the outlives facts visible at this solver's site.
    #[must_use]
    pub fn outlives_environment(&self) -> &OutlivesEnvironment { &self.outlives_environment }

    /// Reduces a value and its descendants until no further step is available.
    ///
    /// Reduction implementations must make progress toward termination. The
    /// outlives constraints that reduction produces are dropped; see
    /// [`Self::normalize_with_outlives`] to keep them.
    pub async fn normalize<T>(&self, value: &T) -> T
    where
        T: Reduce + Clone + PartialEq + Send,
    {
        self.normalize_with_outlives(value).await.0
    }

    /// Like [`Self::normalize`], but also returns the outlives constraints
    /// that reduction produces, from given equalities that match modulo
    /// lifetimes.
    pub async fn normalize_with_outlives<T>(&self, value: &T) -> (T, OutlivesConstraints)
    where
        T: Reduce + Clone + PartialEq + Send,
    {
        let mut normalized = value.clone();
        let mut outlives = OutlivesConstraints::new();
        while let Some((reduced, new_outlives)) =
            normalized.reduce(self.engine(), self.givens()).await
        {
            assert!(reduced != normalized, "reduction must make progress");
            normalized = reduced;
            outlives = outlives.union(new_outlives);
        }
        (normalized, outlives)
    }

    /// Replaces the generator of this solver's inference variables.
    ///
    /// This must be done before any inference variable is created, so that
    /// every variable comes from one generator.
    #[must_use]
    pub fn with_inference_generator(mut self, generator: Box<dyn InferenceGenerator>) -> Self {
        self.inference_generator = generator;
        self
    }

    /// The generator of this solver's inference variables. Callers that
    /// installed their own generator can downcast it through
    /// [`InferenceGenerator::as_any`].
    #[must_use]
    pub fn inference_generator(&self) -> &dyn InferenceGenerator { &*self.inference_generator }

    /// Mutable access to the generator, e.g. to take what a recording
    /// generator collected through [`InferenceGenerator::as_any_mut`].
    #[must_use]
    pub fn inference_generator_mut(&mut self) -> &mut dyn InferenceGenerator {
        &mut *self.inference_generator
    }

    #[must_use]
    pub fn new_inference(&mut self, kind: TyKind) -> Inference {
        self.new_inference_with_constraint(kind, InferenceConstraint::Any)
    }

    #[must_use]
    pub fn new_inference_with_constraint(
        &mut self,
        kind: TyKind,
        constraint: InferenceConstraint,
    ) -> Inference {
        self.inference_generator.generate(kind, constraint)
    }
}

#[cfg(test)]
mod test;
