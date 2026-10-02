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
//! The paths are searched in the location-insensitive constraint graph: the
//! union of the constraints of every point. A universal region is live at
//! every point, so what flows into one at any point stays in it at every
//! other, and the points do not matter. Compared with following the points,
//! this only rejects more when a value is overwritten before it is used
//! again, as NLL does.
//!
//! The search from `'a` stops at each universal region it reaches. A path
//! that goes on through `'b` to `'c` is found again by the search from `'b`,
//! and the environment is transitive, so `'a: 'b` and `'b: 'c` holding means
//! `'a: 'c` does too. This also reports a missing relation once, where it
//! arises, and not again for every region upstream of it.

use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{cfg::Point, ir_function::IRFunction};
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::Solver;
use rayc_type::ty::Ty;

use crate::{
    constraint::LocalizedConstraints,
    diagnostic::{Diagnostic, LifetimeMayNotLiveLongEnough},
};

/// Checks that every relation between two universal regions that the
/// `constraints` of `function` require follows from the outlives environment
/// of `solver`, and returns the ones that do not.
///
/// `solver` must be created at the definition the function belongs to.
pub(crate) fn check_universal_regions(
    function: &IRFunction,
    constraints: &LocalizedConstraints,
    solver: &Solver,
) -> Vec<Diagnostic> {
    let checker = UniversalRegionChecker { function, solver, graph: SubsetGraph::new(constraints) };

    checker.check()
}

/// A constraint `'lesser: 'greater` out of a region `'lesser`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Edge {
    /// The point of the instruction requiring the constraint. It comes
    /// first so that the edges of a region are ordered by where they arise.
    point: Point,

    greater: Interned<Ty>,
}

/// The location-insensitive constraint graph: for each region, the regions
/// it must outlive at any point.
struct SubsetGraph {
    edges: FxHashMap<Interned<Ty>, Vec<Edge>>,
}

impl SubsetGraph {
    /// Builds the graph of the union of `constraints` over every point.
    fn new(constraints: &LocalizedConstraints) -> Self {
        let mut edges = FxHashMap::<_, Vec<Edge>>::default();
        for (lesser, point, greater) in constraints.outlives() {
            edges.entry(lesser.clone()).or_default().push(Edge { point, greater: greater.clone() });
        }

        Self { edges }
    }

    /// Iterates over the universal regions with a constraint out of them, in
    /// a fixed order.
    fn universal_sources(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.edges.keys().filter(|region| is_universal(region))
    }

    /// Iterates over the constraints out of `region`, in a fixed order.
    fn edges_of(&self, region: &Interned<Ty>) -> impl Iterator<Item = &Edge> {
        self.edges.get(region).into_iter().flatten()
    }
}

/// The universal regions a search reached from its source, and how.
struct Reached<'a> {
    /// The universal regions reached, in the order the search found them.
    universals: Vec<&'a Interned<Ty>>,

    /// For each region reached, the region it was reached from and the point
    /// of the constraint between them.
    parents: FxHashMap<&'a Interned<Ty>, (&'a Interned<Ty>, Point)>,
}

impl Reached<'_> {
    /// Returns the points of the constraints on the path the search took to
    /// `region`, from the source of the search onwards.
    fn path_points(&self, region: &Interned<Ty>) -> Vec<Point> {
        let mut points = Vec::new();
        let mut current = region;
        while let Some(&(parent, point)) = self.parents.get(current) {
            points.push(point);
            current = parent;
        }

        points.reverse();
        points
    }
}

/// Checks the universal regions of an IR function for
/// [`check_universal_regions`].
struct UniversalRegionChecker<'a> {
    function: &'a IRFunction,
    solver: &'a Solver,
    graph: SubsetGraph,
}

impl UniversalRegionChecker<'_> {
    /// Checks the relations required of each universal region, and returns
    /// the errors found.
    fn check(&self) -> Vec<Diagnostic> {
        let environment = self.solver.outlives_environment();
        let mut diagnostics = Vec::new();

        for longer in self.graph.universal_sources() {
            let reached = self.reach_from(longer);

            for &shorter in &reached.universals {
                // TODO: a relation with an external region is not known to
                // the environment of the definition. It is a requirement for
                // the creator of the nested function to prove, where it
                // instantiates the external regions, and not an error here.
                if longer.is_external_lifetime() || shorter.is_external_lifetime() {
                    continue;
                }

                if environment.region_outlives(longer, shorter) {
                    continue;
                }

                diagnostics.push(self.explain(longer, shorter, &reached.path_points(shorter)));
            }
        }

        diagnostics
    }

    /// Searches the graph breadth-first from the universal region `source`,
    /// through the regions of the body, up to each universal region.
    fn reach_from<'s>(&'s self, source: &'s Interned<Ty>) -> Reached<'s> {
        let mut reached = Reached { universals: Vec::new(), parents: FxHashMap::default() };
        let mut pending = VecDeque::from_iter([source]);

        while let Some(region) = pending.pop_front() {
            for edge in self.graph.edges_of(region) {
                let greater = &edge.greater;
                if greater == source || reached.parents.contains_key(greater) {
                    continue;
                }
                reached.parents.insert(greater, (region, edge.point));

                // The search from a universal region continues the paths
                // through it; see the module documentation.
                if is_universal(greater) {
                    reached.universals.push(greater);
                } else {
                    pending.push_back(greater);
                }
            }
        }

        reached
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

/// Returns whether `region` is universal to the function mentioning it.
///
/// After [renumbering](crate::renumber), every lifetime a function chooses
/// is a region variable, so any other lifetime is given to it.
const fn is_universal(region: &Ty) -> bool { region.as_region().is_none() }
