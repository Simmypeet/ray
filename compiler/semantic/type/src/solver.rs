use rayc_qbice::TrackedEngine;

use crate::{
    constraint::{Constraint, Step, ty_relate::TyRelatingEnvironment},
    reduce::Reduce,
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{InferenceConstraint, TyKind, inference::Inference},
};

#[cfg(test)]
mod test;

#[derive(Debug, Clone)]
pub struct Solver {
    inference_counter: u64,
    engine: TrackedEngine,
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
            .map(|(head, expected)| Constraint::new_subtype(head.clone(), expected.clone()))
            .collect();

        self.exhaustive_solve(constrs, &TyRelatingEnvironment::TopLevelMatching).await
    }

    /// Solves all constraints, returning the composed substitution.
    ///
    /// Returns `None` if entailment fails or constraints remain after no
    /// further progress can be made.
    pub async fn exhaustive_solve(
        &mut self,
        mut constrs: Vec<Constraint>,
        relate_env: &TyRelatingEnvironment,
    ) -> Option<Subst> {
        let mut subst = Subst::new_empty();
        let mut residual = Vec::<Constraint>::new();

        while let Some(constraint) = constrs.pop() {
            match self.entail_with_relate_env(&constraint, relate_env).await.ok()? {
                Step::Derived(derived) => {
                    constrs.extend(derived.into_iter().map(|derived| derived.constraint));
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
                    if let Some(reduced) = constraint.reduce(&self.engine) {
                        constrs.push(reduced);
                    } else {
                        residual.push(constraint);
                    }
                }
            }
        }

        residual.is_empty().then_some(subst)
    }

    #[must_use]
    pub const fn new(engine: TrackedEngine) -> Self { Self { inference_counter: 0, engine } }

    #[must_use]
    pub const fn engine(&self) -> &TrackedEngine { &self.engine }

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
