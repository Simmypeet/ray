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
    variance::LifetimeVariances,
};

pub mod active_loans;
mod conflict;
pub mod constraint;
pub mod diagnostic;
pub mod live_loans;
pub mod region_liveness;
pub mod renumber;
pub mod variance;

#[cfg(test)]
mod test_util;

/// Borrow checks every IR function of the definition that `ir` lowers, and
/// returns the errors found.
///
/// This expects IR without errors, after the memory analysis has elaborated
/// its drops.
pub async fn borrow_check(ir: &IRFunctionMap, engine: &TrackedEngine) -> Vec<Diagnostic> {
    // Renumbering rewrites the lifetimes of the IR, which is lowered further
    // as type inference left it, so the borrow checker works on a copy.
    let mut ir = ir.clone();
    let _ = Renumbering::renumber(&mut ir, engine).await;
    let variances = LifetimeVariances::compute(&ir, engine).await;
    let mut solver = Solver::new(engine.clone(), ir.def_id()).await;

    let mut diagnostics = Vec::new();
    for (function_id, function) in ir.functions() {
        let captures = ir.captures_for_function(function_id);
        let constraints = LocalizedConstraints::collect(function, captures, &mut solver).await;
        let liveness = RegionLiveness::compute(&ir, function_id).await;
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
