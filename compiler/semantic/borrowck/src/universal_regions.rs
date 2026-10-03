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
//! The environment states nothing of an external lifetime: the function
//! creating the nested function chooses it. So when `'a` or `'b` is one, the
//! relation is not an error, but a [requirement](crate::requirement) for the
//! creator to prove of the region it chooses, where it creates the nested
//! function.
//!
//! The paths are searched in the location-insensitive constraint graph; see
//! [`SubsetGraph`].
//!
//! The search from `'a` stops at each universal region it reaches. A path
//! that goes on through `'b` to `'c` is found again by the search from `'b`,
//! and the environment is transitive, so `'a: 'b` and `'b: 'c` holding means
//! `'a: 'c` does too. This also reports a missing relation once, where it
//! arises, and not again for every region upstream of it. The same goes for a
//! path through an external region: the creator is required each step of it,
//! which it then checks as a path through the region it chooses.

use qbice::storage::intern::Interned;
use rayc_ir::ir_function::IRFunction;
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::Solver;
use rayc_type::ty::Ty;

use crate::{
    diagnostic::{Diagnostic, LifetimeMayNotLiveLongEnough},
    requirement::ExternalRequirements,
    subset_graph::{Reached, SubsetGraph},
};

/// Checks that every relation between two universal regions that the
/// constraints of `function`, gathered in `graph`, require follows from the
/// outlives environment of `solver`, and returns the ones that do not.
///
/// A relation with an external lifetime is added to `requirements` instead,
/// for the function creating `function` to prove.
///
/// `solver` must be created at the definition the function belongs to.
pub(crate) fn check_universal_regions(
    function: &IRFunction,
    graph: &SubsetGraph,
    solver: &Solver,
    requirements: &mut ExternalRequirements,
) -> Vec<Diagnostic> {
    let checker = UniversalRegionChecker { function, solver, graph };

    checker.check(requirements)
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
    /// the errors found. The relations left for the creator of the function
    /// to prove are added to `requirements`.
    fn check(&self, requirements: &mut ExternalRequirements) -> Vec<Diagnostic> {
        let environment = self.solver.outlives_environment();
        let mut diagnostics = Vec::new();

        for longer in self.graph.universals() {
            // Nearly every function requires nothing it may not assume,
            // which the closure of the graph tells without a search.
            if self
                .graph
                .reachable_universals(longer)
                .all(|shorter| environment.region_outlives(longer, shorter))
            {
                continue;
            }

            // Search for the relations that do not hold, and for the
            // constraints that require them.
            let reached = self.graph.reach_universals_from(longer);

            for shorter in reached.universals() {
                if environment.region_outlives(longer, shorter) {
                    continue;
                }

                let source = self.source(&reached, shorter);

                // The creator of a nested function chooses its external
                // lifetimes, so only the creator can tell what they outlive.
                if longer.is_external_lifetime() || shorter.is_external_lifetime() {
                    requirements.require(longer.clone(), shorter.clone(), source.span);
                } else {
                    diagnostics.push(Self::explain(longer, shorter, source));
                }
            }
        }

        diagnostics
    }

    /// Returns where the relation between the source of the search `reached`
    /// and the universal region `shorter` it reached arises: at the last
    /// constraint of the path between them, where the value flows into
    /// `shorter`, and at the first, where the value comes from.
    ///
    /// # Panics
    ///
    /// Panics if no constraint of the path has a source. A constraint with a
    /// universal region is required by an instruction reading or writing a
    /// value, which always has one.
    fn source(&self, reached: &Reached<'_>, shorter: &Interned<Ty>) -> RequirementSource {
        let mut spans = reached.path_spans(shorter, self.function).into_iter().flatten();

        let origin_span =
            spans.next().expect("a constraint path between universal regions should have a source");
        let span = spans.next_back().unwrap_or(origin_span);

        RequirementSource { span, origin_span }
    }

    /// Describes the unproven requirement `longer: shorter`, which arises at
    /// `source`.
    ///
    /// The error points at the last constraint, where the value flows into
    /// `shorter`, and at the first, where the value of `longer` comes from,
    /// when that is somewhere else.
    fn explain(
        longer: &Interned<Ty>,
        shorter: &Interned<Ty>,
        source: RequirementSource,
    ) -> Diagnostic {
        Diagnostic::LifetimeMayNotLiveLongEnough(LifetimeMayNotLiveLongEnough::new(
            source.span,
            longer.clone(),
            shorter.clone(),
            (source.origin_span != source.span).then_some(source.origin_span),
        ))
    }
}

/// Where a relation between two universal regions arises in a function.
#[derive(Debug, Clone, Copy)]
struct RequirementSource {
    /// The source of the last constraint of the path between the two regions:
    /// where the value of the longer lifetime flows into the shorter one.
    span: RelativeSpan,

    /// The source of the first constraint of the path: where the value of
    /// the longer lifetime comes from.
    origin_span: RelativeSpan,
}
