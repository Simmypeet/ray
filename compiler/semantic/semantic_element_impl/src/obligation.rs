//! Checks deferred obligations emitted while constructing semantic elements.
//!
//! Signatures have no inference variables, so every obligation is checked on
//! its own in a single pass, without a shared constraint system. An obligation
//! that relates types may hold only if certain lifetimes outlive others; those
//! outlives constraints are checked next, and each one that does not hold is
//! reported against the obligation that required it.

use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{
    Obligation, PredicateConstraint, PredicateObligation, ReferenceWf, RelationOrigin,
    TraitRefCheck, UnsatisfiedRelationOutlives,
};
use rayc_solver::{Solver, ty_relate::Step};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    constraint::outlives::{OutlivesConstraint, OutlivesConstraints},
    where_clause::OutlivesPredicate,
};

/// Checks obligations after construction, rendering a diagnostic for each
/// one that fails.
pub(crate) async fn solve_obligations(
    obligations: Vec<Obligation>,
    site: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Vec<Rendered<ByteIndex>> {
    // Query and instantiate where clauses only after all semantic elements are
    // available, so resolution can refer to the clause currently being built.
    let mut expanded = Vec::new();
    for obligation in obligations {
        ExpandedObligation::expand_into(obligation, engine, &mut expanded).await;
    }

    // Check each obligation on its own against the site's givens.
    let mut solver = Solver::new(engine.clone(), site).await;
    let mut diagnostics = Vec::new();
    for obligation in &expanded {
        obligation.check(&mut solver, &mut diagnostics).await;
    }

    diagnostics
}

/// One obligation after where clauses are instantiated, kept whole for its
/// diagnostic.
enum ExpandedObligation {
    TraitRefCheck(TraitRefCheck),
    Predicate(PredicateObligation),
    ReferenceWf(ReferenceWf),
}

impl ExpandedObligation {
    /// Expands `obligation` into `expanded`, instantiating the where clauses
    /// it refers to.
    async fn expand_into(obligation: Obligation, engine: &TrackedEngine, expanded: &mut Vec<Self>) {
        match obligation {
            Obligation::TraitRefCheck(check) => expanded.push(Self::TraitRefCheck(check)),
            Obligation::WfCheck(check) => expanded
                .extend(check.predicate_obligations(engine).await.into_iter().map(Self::Predicate)),
            Obligation::ReferenceWf(check) => expanded.push(Self::ReferenceWf(check)),
        }
    }

    /// Checks this obligation, pushing its diagnostic if it fails, or one for
    /// each outlives constraint its type relations require that does not
    /// hold.
    async fn check(&self, solver: &mut Solver, diagnostics: &mut Vec<Rendered<ByteIndex>>) {
        let engine = solver.engine().clone();

        let Some(outlives) = self.solve(solver).await else {
            diagnostics.push(self.report(&engine).await);
            return;
        };

        // Lifetimes never decide whether types relate, so the obligation
        // holds and the lifetimes it relates are reported separately.
        let Some(origin) = self.relation_origin() else {
            return;
        };
        for constraint in unsatisfied_outlives(&outlives, solver).await {
            let diagnostic = UnsatisfiedRelationOutlives::new(constraint, origin.clone());
            diagnostics.push(diagnostic.report(&engine).await);
        }
    }

    /// Returns the outlives constraints required by the types this obligation
    /// relates, or `None` if the obligation does not hold.
    async fn solve(&self, solver: &mut Solver) -> Option<OutlivesConstraints> {
        match self {
            Self::TraitRefCheck(check) => {
                let derived = match solver.entail_instance_trait_ref(check.constraint()).await {
                    Ok(Step::Derived(derived)) => derived,
                    Ok(Step::NoProgress) | Err(_) => return None,
                    Ok(Step::Subst(_) | Step::Generalized { .. }) => {
                        unreachable!("trait checks only derive type relations")
                    }
                };
                let relations = derived.into_iter().map(|derived| derived.ty_relate).collect();
                solver.solve_without_unify(relations).await
            }
            Self::Predicate(predicate) => match predicate.constraint() {
                PredicateConstraint::TyRelate(relation) => {
                    solver.solve_without_unify(vec![relation]).await
                }
                PredicateConstraint::Marker(marker) => {
                    solver.entails_marker_predicate(marker).await.then(OutlivesConstraints::new)
                }
                PredicateConstraint::Outlives(predicate) => {
                    solver.entails_outlives(&predicate).await.then(OutlivesConstraints::new)
                }
            },
            Self::ReferenceWf(check) => {
                solver.entails_outlives(&check.predicate()).await.then(OutlivesConstraints::new)
            }
        }
    }

    /// Returns the obligation as the origin of a lifetime relation, if
    /// relating its types can require one.
    fn relation_origin(&self) -> Option<RelationOrigin> {
        match self {
            Self::TraitRefCheck(check) => Some(RelationOrigin::TraitRefCheck(check.clone())),
            Self::Predicate(predicate) => Some(RelationOrigin::Predicate(predicate.clone())),
            Self::ReferenceWf(_) => None,
        }
    }

    /// Renders the failed obligation.
    async fn report(&self, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::TraitRefCheck(check) => check.report(engine).await,
            Self::Predicate(predicate) => predicate.report(engine).await,
            Self::ReferenceWf(check) => check.report(engine).await,
        }
    }
}

/// Returns the constraints among `outlives` that do not follow from the
/// solver's outlives facts.
async fn unsatisfied_outlives(
    outlives: &OutlivesConstraints,
    solver: &mut Solver,
) -> Vec<OutlivesConstraint> {
    let mut unsatisfied = Vec::new();
    for constraint in outlives.iter() {
        let predicate =
            OutlivesPredicate::new(constraint.lesser().clone(), constraint.greater().clone());
        if !solver.entails_outlives(&predicate).await {
            unsatisfied.push(constraint.clone());
        }
    }

    // The set has no order, so the diagnostics are sorted to be stable.
    unsatisfied
}
