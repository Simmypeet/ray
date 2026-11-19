use bon::bon;
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

pub use marker_entailment::{CoroutineSavedLocalsVisibility, MarkerEntailment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TyRelatingEnvironment {
    /// Normal type relation, where inference variables on either side may be
    /// bound to types of the same kind.
    Normal,

    /// One-way, invariant matching that may bind only polymorphic variables on
    /// the lesser side, for matching instance heads.
    TopLevelMatching,

    /// One-way, invariant matching that may bind only external lifetimes on the
    /// lesser side, for matching a nested IR function's interface.
    InterfaceMatching,
}

impl TyRelatingEnvironment {
    /// Returns whether this environment matches one side against the other,
    /// which relates the two invariantly and never generalizes.
    #[must_use]
    pub const fn is_matching(&self) -> bool {
        match self {
            Self::Normal => false,
            Self::TopLevelMatching | Self::InterfaceMatching => true,
        }
    }
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
    /// Returns whether two types are equal without binding any variables, for
    /// use once inference has finished.
    pub async fn eq_without_unify(&mut self, left: &Interned<Ty>, right: &Interned<Ty>) -> bool {
        self.relate_without_unify(left, right).await.is_some()
    }

    /// Returns the outlives constraints that make two types equal without
    /// binding any variables, or `None` if they differ.
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

    /// Like [`Self::relate_without_unify`], but for trait references.
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

    /// Solves relations without binding any variables, returning the outlives
    /// constraints they require, or `None` if any fails.
    pub async fn solve_without_unify(
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
                self.entails_marker_predicate(predicate.clone()).await.holds()
            }
            PredicateKind::Outlives(predicate) => self.entails_outlives(predicate).await,
        }
    }

    /// Matches an instance head against an expected trait reference, binding
    /// only the head's polymorphic variables.
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

    /// Matches a marker implementation head against a concrete type, which
    /// needs no outlives constraints since such heads ignore lifetimes.
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

    /// Matches a nested IR function's interface types against its creator's,
    /// returning the creator's lifetime for each external lifetime.
    pub async fn interface_match(
        &mut self,
        interface: impl IntoIterator<Item = (Interned<Ty>, Interned<Ty>)>,
    ) -> Option<Solution> {
        let constrs = interface
            .into_iter()
            .map(|(interface, created)| TyRelate::new_invariant(interface, created))
            .collect();

        self.exhaustive_solve(constrs, &TyRelatingEnvironment::InterfaceMatching).await
    }

    /// Solves all constraints to a fixpoint, returning the composed
    /// substitution and required outlives constraints, or `None` on
    /// failure.
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

    /// Composes `subst` into the solved substitution, requeueing residual
    /// constraints it changes.
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

    /// Creates a solver without a site or predicates, for monomorphized code
    /// and unit tests.
    pub async fn without_givens(engine: TrackedEngine) -> Self {
        Self::with_givens(engine, GlobalSymbolID::default(), []).await
    }

    /// Creates a solver with exactly the supplied predicates, for entailment
    /// checks that must exclude the site's own where clause.
    pub async fn with_givens(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        givens: impl IntoIterator<Item = PredicateKind>,
    ) -> Self {
        let givens = engine.intern_unsized(givens.into_iter().collect::<Vec<_>>());
        Self::builder().engine(engine).site(site).givens(givens).build().await
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
        Self::builder().engine(engine).site(site).limits(limits).build().await
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
    pub const fn outlives_environment(&self) -> &Interned<OutlivesEnvironment> {
        &self.outlives_environment
    }

    /// Reduces a value to normal form, dropping the outlives constraints; see
    /// [`Self::normalize_with_outlives`] to keep them.
    pub async fn normalize<T>(&self, value: &T) -> T
    where
        T: Reduce + Clone + PartialEq + Send,
    {
        self.normalize_with_outlives(value).await.0
    }

    /// Like [`Self::normalize`], but also returns the outlives constraints
    /// that reduction produces.
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

    /// The generator of this solver's inference variables, which callers can
    /// downcast through [`InferenceGenerator::as_any`].
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

#[bon]
impl Solver {
    /// Creates a solver that sees every predicate visible at `site`, or exactly
    /// `givens` when supplied.
    #[builder]
    pub async fn builder(
        engine: TrackedEngine,
        site: GlobalSymbolID,
        #[builder(default = InstanceResolutionLimits::default())] limits: InstanceResolutionLimits,
        #[builder(default = Box::new(CountingInferenceGenerator::default()))]
        inference_generator: Box<dyn InferenceGenerator>,
        givens: Option<Interned<[PredicateKind]>>,
        #[builder(default = CoroutineSavedLocalsVisibility::Visible)]
        coroutine_visiblity: CoroutineSavedLocalsVisibility,
    ) -> Self {
        let (givens, outlives_environment) = match givens {
            Some(givens) => {
                let outlives_environment = engine.intern(
                    OutlivesEnvironment::new(outlives_givens(&givens).cloned(), &engine).await,
                );
                (givens, outlives_environment)
            }
            None => (engine.get_givens(site).await, engine.get_outlives_environment(site).await),
        };

        Self {
            inference_generator,
            site,
            instance_resolution: InstanceResolutionState::new(limits),
            marker_entailment: MarkerEntailmentState::new(coroutine_visiblity),
            givens,
            outlives_environment,
            engine,
        }
    }
}

#[cfg(test)]
mod test;
