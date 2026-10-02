//! Checks the relations an IR function requires between its universal
//! regions.
//!
//! A universal region is one the function is given rather than one it
//! chooses: `'static`, a lifetime parameter of the definition, or an external
//! lifetime of a nested function. The function may only assume of them what
//! its outlives environment states: its where clause and the bounds implied
//! by its signature.
//!
//! When the constraints of the body lead from a universal region `'a` to a
//! universal region `'b`, through any number of regions of the body, the
//! function requires `'a: 'b`. That is an error unless the environment
//! entails it.
//!
//! The paths are searched in the location-insensitive constraint graph; see
//! [`SubsetGraph`].
//!
//! The search from `'a` stops at each universal region it reaches. A path
//! that goes on through `'b` to `'c` is found again by the search from `'b`,
//! and the environment is transitive, so `'a: 'b` and `'b: 'c` holding means
//! `'a: 'c` does too. This also reports a missing relation once, where it
//! arises, and not again for every region upstream of it.

use qbice::storage::intern::Interned;
use rayc_ir::{cfg::Point, ir_function::IRFunction};
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::Solver;
use rayc_type::ty::Ty;

use crate::{
    diagnostic::{Diagnostic, LifetimeMayNotLiveLongEnough},
    subset_graph::SubsetGraph,
};

/// Checks that every relation between two universal regions that the
/// constraints of `function`, gathered in `graph`, require follows from the
/// outlives environment of `solver`, and returns the ones that do not.
///
/// `solver` must be created at the definition the function belongs to.
pub(crate) fn check_universal_regions(
    function: &IRFunction,
    graph: &SubsetGraph,
    solver: &Solver,
) -> Vec<Diagnostic> {
    let checker = UniversalRegionChecker { function, solver, graph };

    checker.check()
}

/// Checks the universal regions of an IR function for
/// [`check_universal_regions`].
struct UniversalRegionChecker<'a> {
    function: &'a IRFunction,
    solver: &'a Solver,
    graph: &'a SubsetGraph,
}

impl UniversalRegionChecker<'_> {
    /// Checks the relations required of each universal region, and returns
    /// the errors found.
    fn check(&self) -> Vec<Diagnostic> {
        let environment = self.solver.outlives_environment();
        let mut diagnostics = Vec::new();

        // TODO: a relation with an external region is not known to the
        // environment of the definition. It is a requirement for the creator
        // of the nested function to prove, where it instantiates the external
        // regions, and not an error here.
        let holds = |longer: &Interned<Ty>, shorter: &Interned<Ty>| {
            longer.is_external_lifetime()
                || shorter.is_external_lifetime()
                || environment.region_outlives(longer, shorter)
        };

        for longer in self.graph.universals() {
            // Nearly every function requires nothing it may not assume,
            // which the closure of the graph tells without a search.
            if self.graph.reachable_universals(longer).all(|shorter| holds(longer, shorter)) {
                continue;
            }

            // Search for the relations that do not hold, and for the
            // constraints that require them.
            let reached = self.graph.reach_universals_from(longer);

            for shorter in reached.universals() {
                if holds(longer, shorter) {
                    continue;
                }

                diagnostics.push(self.explain(longer, shorter, &reached.path_points(shorter)));
            }
        }

        diagnostics
    }

    /// Describes the unproven requirement `longer: shorter`, which arises
    /// along the constraints at `path`, from `longer` onwards.
    ///
    /// The error points at the last constraint, where the value flows into
    /// `shorter`, and at the first, where the value of `longer` comes from,
    /// when that is somewhere else.
    ///
    /// # Panics
    ///
    /// Panics if no constraint of `path` has a source. A constraint with a
    /// universal region is required by an instruction reading or writing a
    /// value, which always has one.
    fn explain(&self, longer: &Interned<Ty>, shorter: &Interned<Ty>, path: &[Point]) -> Diagnostic {
        let mut spans = path.iter().filter_map(|&point| self.function.point_span(point));

        let origin_span: RelativeSpan =
            spans.next().expect("a constraint path between universal regions should have a source");
        let span = spans.next_back().unwrap_or(origin_span);

        Diagnostic::LifetimeMayNotLiveLongEnough(LifetimeMayNotLiveLongEnough::new(
            span,
            longer.clone(),
            shorter.clone(),
            (origin_span != span).then_some(origin_span),
        ))
    }
}
