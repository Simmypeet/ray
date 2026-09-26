//! Solves deferred obligations emitted while constructing semantic elements.

use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{
    Obligation, PredicateConstraint, PredicateObligation, ReferenceWf, TraitRefCheck,
};
use rayc_solver::ty_relate::Step;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
    where_clause::OutlivesPredicate,
};

/// One obligation after where clauses are instantiated, kept whole for its
/// diagnostic.
enum ExpandedObligation {
    TraitRefCheck(TraitRefCheck),
    Predicate(PredicateObligation),
    ReferenceWf(ReferenceWf),
}

impl ExpandedObligation {
    /// Returns the outlives requirement this obligation stands for, if any.
    fn outlives_predicate(&self) -> Option<OutlivesPredicate> {
        match self {
            Self::Predicate(predicate) => match predicate.constraint() {
                PredicateConstraint::Outlives(predicate) => Some(predicate),
                PredicateConstraint::TyRelate(_) | PredicateConstraint::Marker(_) => None,
            },
            Self::ReferenceWf(check) => Some(check.predicate()),
            Self::TraitRefCheck(_) => None,
        }
    }

    /// Renders the failed obligation after applying `subst`.
    async fn report(&self, subst: &Subst, engine: &TrackedEngine) -> Rendered<ByteIndex> {
        match self {
            Self::TraitRefCheck(check) => {
                check.apply_subst_or_clone(subst, engine).report(engine).await
            }
            Self::Predicate(predicate) => {
                predicate.apply_subst_or_clone(subst, engine).report(engine).await
            }
            Self::ReferenceWf(check) => {
                check.apply_subst_or_clone(subst, engine).report(engine).await
            }
        }
    }
}

/// Solves obligations together after construction, retaining each source
/// obligation for diagnostics.
pub(crate) async fn solve_obligations(
    obligations: Vec<Obligation>,
    site: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Vec<Rendered<ByteIndex>> {
    // TODO: Currently, this works. But we'd have to find a better way to organize
    // this!

    let mut solver = rayc_solver::Solver::new(engine.clone(), site).await;
    let mut expanded = Vec::new();
    let mut constraints = Vec::new();
    let mut failed = std::collections::BTreeSet::new();

    // Query and instantiate where clauses only after all semantic elements are
    // available, so resolution can refer to the clause currently being built.
    for obligation in obligations {
        match obligation {
            Obligation::TraitRefCheck(check) => {
                expanded.push(ExpandedObligation::TraitRefCheck(check));
            }
            Obligation::WfCheck(check) => expanded.extend(
                check
                    .predicate_obligations(engine)
                    .await
                    .into_iter()
                    .map(ExpandedObligation::Predicate),
            ),
            Obligation::ReferenceWf(check) => {
                expanded.push(ExpandedObligation::ReferenceWf(check));
            }
        }
    }

    // Lower the expanded obligations into constraints while retaining each
    // individual predicate for diagnostics.
    for (index, obligation) in expanded.iter().enumerate() {
        match obligation {
            ExpandedObligation::TraitRefCheck(check) => {
                match solver.entail_instance_trait_ref(check.constraint()).await {
                    Ok(Step::Derived(derived)) => constraints
                        .extend(derived.into_iter().map(|derived| (index, derived.ty_relate))),
                    Ok(Step::NoProgress) | Err(_) => {
                        failed.insert(index);
                    }
                    Ok(Step::Subst(_)) => unreachable!("trait checks only derive type relations"),
                }
            }
            ExpandedObligation::Predicate(predicate) => {
                if let PredicateConstraint::TyRelate(constraint) = predicate.constraint() {
                    constraints.push((index, constraint));
                }
            }
            ExpandedObligation::ReferenceWf(_) => {}
        }
    }

    // Solve every derived relation as one set so substitutions can discharge
    // later obligations.
    let mut subst = Subst::new_empty();
    let mut residual = Vec::new();
    while let Some((index, constraint)) = constraints.pop() {
        match solver.entail_ty_relate(&constraint).await {
            Ok(Step::Derived(derived)) => {
                constraints.extend(derived.into_iter().map(|derived| (index, derived.ty_relate)));
            }
            Ok(Step::Subst(new)) => {
                subst.compose(&new, engine);
                constraints.append(&mut residual);
                for (_, constraint) in &mut constraints {
                    constraint.apply_in_place(&new, engine);
                }
            }
            Ok(Step::NoProgress) => {
                if let Some(reduced) = constraint.reduce(solver.engine(), solver.givens()).await {
                    constraints.push((index, reduced));
                } else {
                    residual.push((index, constraint));
                }
            }
            Err(_) => {
                failed.insert(index);
            }
        }
    }
    failed.extend(residual.into_iter().map(|(index, _)| index));

    // Type equalities run first so marker goals observe the final substitution
    // produced by the complete obligation set.
    for (index, obligation) in expanded.iter().enumerate() {
        let ExpandedObligation::Predicate(predicate) = obligation else {
            continue;
        };
        let PredicateConstraint::Marker(marker) = predicate.constraint() else {
            continue;
        };
        let marker = marker.apply_subst_or_clone(&subst, engine);
        if !solver.entails_marker_predicate(marker).await {
            failed.insert(index);
        }
    }

    // Outlives obligations run last, against the site's outlives givens and
    // implied bounds.
    failed.extend(failed_outlives(&expanded, &subst, &mut solver).await);

    // Report the original obligation after applying every substitution learned
    // while solving the complete set.
    let mut rendered = Vec::new();
    for index in failed {
        rendered.push(expanded[index].report(&subst, engine).await);
    }
    rendered
}

/// Returns the indices of the outlives obligations that do not follow from the
/// solver's outlives facts.
///
/// They only concern named lifetimes, so an obligation that still mentions an
/// inference variable is left to later checks.
async fn failed_outlives(
    expanded: &[ExpandedObligation],
    subst: &Subst,
    solver: &mut rayc_solver::Solver,
) -> Vec<usize> {
    let mut failed = Vec::new();
    for (index, obligation) in expanded.iter().enumerate() {
        let Some(predicate) = obligation.outlives_predicate() else {
            continue;
        };

        let predicate = predicate.apply_subst_or_clone(subst, solver.engine());

        if !solver.entails_outlives(&predicate).await {
            failed.push(index);
        }
    }
    failed
}
