//! Solves deferred obligations emitted while constructing semantic elements.

use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_resolution::{Obligation, PredicateConstraint, PredicateObligation, TraitRefCheck};
use rayc_solver::ty_relate::Step;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    reduce::Reduce,
    subst::{Subst, Substitutable},
};

/// Solves obligations together after construction, retaining each source
/// obligation for diagnostics.
pub(crate) async fn solve_obligations(
    obligations: Vec<Obligation>,
    site: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Vec<Rendered<ByteIndex>> {
    enum ExpandedObligation {
        TraitRefCheck(TraitRefCheck),
        Predicate(PredicateObligation),
    }

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

    // Report the original obligation after applying every substitution learned
    // while solving the complete set.
    let mut rendered = Vec::new();
    for index in failed {
        match &expanded[index] {
            ExpandedObligation::TraitRefCheck(check) => {
                rendered.push(check.apply_subst_or_clone(&subst, engine).report(engine).await);
            }
            ExpandedObligation::Predicate(predicate) => {
                rendered.push(predicate.apply_subst_or_clone(&subst, engine).report(engine).await);
            }
        }
    }
    rendered
}
