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
    requirement::{ExternalRequirements, NestedRequirements},
    subset_graph::SubsetGraph,
    type_test::check_type_tests,
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
pub mod requirement;
mod subset_graph;
mod type_test;
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
/// Each function is checked on its own, after the nested functions it
/// creates: what a nested function requires of the lifetimes in its interface
/// is required of its creator, where the creator creates it; see
/// [`requirement`].
///
/// The lifetimes of `ir` are renumbered in place for the check and left that
/// way: the caller is expected to erase the lifetimes of `ir` afterwards.
pub async fn borrow_check(ir: &mut IRFunctionMap, engine: &TrackedEngine) -> Vec<Diagnostic> {
    // Give every lifetime the borrow checker chooses its own region.
    let _ = Renumbering::renumber(ir, engine).await;
    let variances = LifetimeVariances::compute(ir, engine).await;
    let mut solver = Solver::new(engine.clone(), ir.def_id()).await;

    let mut diagnostics = Vec::new();
    let mut nested = NestedRequirements::default();

    for function_id in ir.functions_innermost_first() {
        let function = ir.get_function(function_id);
        let captures = ir.captures_for_function(function_id);
        let effect = ir.effect_of(function_id, engine).await;
        let constraints =
            LocalizedConstraints::collect(ir, function_id, &effect, &nested, &mut solver).await;

        // What the function requires of its universal regions, and of the
        // types that must outlive them, holds at every point or at none, so
        // it is checked on the constraints alone. What it requires of its
        // external lifetimes is left for its creator to prove.
        let graph = SubsetGraph::new(&constraints, solver.outlives_environment());
        let mut requirements = ExternalRequirements::default();
        diagnostics.extend(check_universal_regions(function, &graph, &solver, &mut requirements));
        diagnostics.extend(
            check_type_tests(function, &constraints, &graph, &mut solver, &mut requirements).await,
        );

        // Only a nested function has external lifetimes, and a creator to
        // require something of.
        debug_assert!(
            function_id != ir.root_id() || requirements.is_empty(),
            "the definition function should require nothing of a creator"
        );
        nested.record(function_id, requirements);

        let liveness = RegionLiveness::compute(ir, function_id).await;
        let live_loans = LiveLoans::compute(function, &constraints, &liveness, &variances);
        let activity = LoanActivity::compute(function, &constraints, &live_loans).await;

        let traversal = Traversal::new(function, &constraints, &liveness, &variances);
        diagnostics.extend(
            check_conflicts(
                function,
                captures,
                &constraints,
                &liveness,
                &traversal,
                &activity,
                &mut solver,
            )
            .await,
        );
    }

    diagnostics
}
