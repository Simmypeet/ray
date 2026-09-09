use qbice::storage::intern::Interned;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::where_clause::PredicateKind;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::ty_relate::TyRelate,
    reduce::Reduce,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{InferenceConstraint, Ty, TyKind, inference::Inference},
};

use crate::{
    givens::get_givens,
    solver::instance_resolution_state::{InstanceResolutionLimits, InstanceResolutionState},
    ty_relate::Step,
};

mod instance_resolution_state;

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
    givens: Interned<[PredicateKind]>,
}

impl Solver {
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
                    if let Some(reduced) = constraint.reduce(&self.engine).await {
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
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self {
        Self {
            inference_counter: 0,
            givens: engine.intern_unsized([]),
            engine,
            site: GlobalSymbolID::default(),
            instance_resolution: InstanceResolutionState::new(InstanceResolutionLimits::default()),
        }
    }

    pub async fn new_at_site(engine: TrackedEngine, site: GlobalSymbolID) -> Self {
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
        }
    }

    /// Performs one ordinary reduction step, falling back to the first visible
    /// equality whose left operand is structurally equal to `ty`.
    pub async fn reduce(&self, ty: &Interned<Ty>) -> Option<Interned<Ty>> {
        if let Some(reduced) = ty.reduce(&self.engine).await {
            return Some(reduced);
        }

        self.givens.iter().find_map(|predicate| match predicate {
            PredicateKind::AssociatedTypeEquality(equality) => {
                (equality.left() == ty).then(|| equality.right().clone())
            }
        })
    }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

    pub const fn site(&self) -> GlobalSymbolID { self.site }

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
