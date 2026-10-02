//! Borrow checking of the IR functions of a definition.

use rayc_ir::ir_function::IRFunctionMap;
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;

use crate::{
    active_loans::LoanActivity,
    conflict::check_conflicts,
    constraint::LocalizedConstraints,
    diagnostic::Diagnostic,
    live_loans::{LiveLoans, Traversal},
    region_liveness::RegionLiveness,
    renumber::Renumbering,
    universal_regions::check_universal_regions,
    variance::LifetimeVariances,
};

pub mod active_loans;
mod conflict;
pub mod constraint;
pub mod diagnostic;
pub mod live_loans;
pub mod region_liveness;
pub mod renumber;
mod universal_regions;
pub mod variance;

#[cfg(test)]
mod test_util;

/// Borrow checks every IR function of the definition that `ir` lowers, and
/// returns the errors found.
///
/// This expects IR without errors, after the memory analysis has elaborated
/// its drops.
///
/// The lifetimes of `ir` are renumbered in place for the check and left that
/// way: the caller is expected to erase the lifetimes of `ir` afterwards.
pub async fn borrow_check(ir: &mut IRFunctionMap, engine: &TrackedEngine) -> Vec<Diagnostic> {
    // Give every lifetime the borrow checker chooses its own region.
    let _ = Renumbering::renumber(ir, engine).await;
    let variances = LifetimeVariances::compute(ir, engine).await;
    let mut solver = Solver::new(engine.clone(), ir.def_id()).await;

    let mut diagnostics = Vec::new();
    for (function_id, function) in ir.functions() {
        let captures = ir.captures_for_function(function_id);
        let constraints = LocalizedConstraints::collect(function, captures, &mut solver).await;

        // The relations between universal regions hold at every point or at
        // none, so they are checked on the constraints alone.
        diagnostics.extend(check_universal_regions(function, &constraints, &solver));

        let liveness = RegionLiveness::compute(ir, function_id).await;
        let live_loans = LiveLoans::compute(function, &constraints, &liveness, &variances);
        let activity = LoanActivity::compute(function, &constraints, &live_loans).await;

        let traversal = Traversal::new(function, &constraints, &liveness, &variances);
        diagnostics.extend(check_conflicts(
            function,
            &constraints,
            &liveness,
            &traversal,
            &activity,
        ));
    }

    diagnostics
}
