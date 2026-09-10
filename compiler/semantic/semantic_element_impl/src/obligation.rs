//! Solves deferred obligations emitted while constructing semantic elements.

use rayc_diagnostic::{ByteIndex, Rendered, Report};
use rayc_qbice::TrackedEngine;
use rayc_resolution::Obligation;
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
    let mut solver = rayc_solver::Solver::new(engine.clone(), site).await;
    let mut constraints = Vec::new();
    let mut failed = std::collections::BTreeSet::new();

    // Expand obligations only after all semantic elements are available.
    for (index, obligation) in obligations.iter().enumerate() {
        match obligation {
            Obligation::TraitRefCheck(check) => {
                match solver.entail_instance_trait_ref(check.constraint()).await {
                    Ok(Step::Derived(derived)) => constraints
                        .extend(derived.into_iter().map(|derived| (index, derived.ty_relate))),
                    Ok(Step::NoProgress) | Err(_) => {
                        failed.insert(index);
                    }
                    Ok(Step::Subst(_)) => unreachable!("trait checks only derive type relations"),
                }
            }
            Obligation::Predicate(predicate) => {
                constraints.push((index, predicate.constraint()));
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

    // Report the original obligation after applying every substitution learned
    // while solving the complete set.
    let mut rendered = Vec::new();
    for index in failed {
        match &obligations[index] {
            Obligation::TraitRefCheck(check) => {
                rendered.push(check.apply_subst_or_clone(&subst, engine).report(engine).await);
            }
            Obligation::Predicate(predicate) => {
                rendered.push(predicate.apply_subst_or_clone(&subst, engine).report(engine).await);
            }
        }
    }
    rendered
}
