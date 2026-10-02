//! The location-insensitive constraint graph of an IR function.
//!
//! This is the union of the constraints of every point: for each region, the
//! regions it must outlive somewhere in the body. A universal region is live
//! at every point, so what flows into one at any point stays in it at every
//! other, and the points do not matter to a question about universal regions.
//! Compared with following the points, the graph only relates more regions
//! when a value is overwritten before it is used again, as NLL does.
//!
//! Two transitive closures of the graph are computed once, and answer what
//! outlives what:
//!
//! - the closure of the constraints alone tells what the body requires, such as
//!   which universal regions a region of the body must outlive. Whether the
//!   function may assume that is then asked of the environment.
//! - the closure of the constraints together with what the environment states
//!   between universal regions tells what holds in a function without errors,
//!   such as whether two regions are the same lifetime. A type test asks it.
//!
//! A closure does not tell which constraints lead from one region to another,
//! so the path an error points at is searched for in the graph, only once a
//! closure has shown that there is an error to report.

use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::cfg::Point;
use rayc_solver::outlives::{OutlivesEnvironment, RegionRelation};
use rayc_transitive_closure::TransitiveClosure;
use rayc_type::ty::Ty;

use crate::constraint::LocalizedConstraints;

/// A constraint `'lesser: 'greater` out of a region `'lesser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edge {
    /// The point of the instruction requiring the constraint.
    point: Point,

    /// The index of `'greater` in the graph.
    greater: usize,
}

/// Numbers the regions of a [`SubsetGraph`] as they are first met.
#[derive(Default)]
struct RegionNumbering {
    /// The regions numbered so far, each at its index.
    regions: Vec<Interned<Ty>>,

    /// The index of each region in `regions`.
    indices: FxHashMap<Interned<Ty>, usize>,
}

impl RegionNumbering {
    /// Returns the index of `region`, numbering it first if it is new.
    fn index_of(&mut self, region: &Interned<Ty>) -> usize {
        if let Some(&index) = self.indices.get(region) {
            return index;
        }

        let index = self.regions.len();
        self.regions.push(region.clone());
        self.indices.insert(region.clone(), index);

        index
    }
}

/// The location-insensitive constraint graph: for each region, the regions
/// it must outlive at any point.
pub(crate) struct SubsetGraph {
    /// The regions with a constraint into or out of them, and the universal
    /// regions the environment mentions. A region is referred to by its
    /// index here.
    regions: Vec<Interned<Ty>>,

    /// The index of each region in `regions`.
    indices: FxHashMap<Interned<Ty>, usize>,

    /// The constraints out of each region.
    edges: Vec<Vec<Edge>>,

    /// The indices of the universal regions.
    universals: Vec<usize>,

    /// The index of `'static`.
    static_region: usize,

    /// `required.has_path(a, b)` means that the constraints of the body
    /// require region `a` to outlive region `b`. Every region has a path to
    /// itself.
    required: TransitiveClosure,

    /// `implied.has_path(a, b)` means that region `a` outlives region `b`
    /// when the constraints of the body hold, given what the environment
    /// states between universal regions.
    implied: TransitiveClosure,
}

impl SubsetGraph {
    /// Builds the graph of the union of `constraints` over every point.
    ///
    /// `environment` must be the outlives environment of the definition the
    /// function belongs to.
    pub(crate) fn new(
        constraints: &LocalizedConstraints,
        environment: &OutlivesEnvironment,
    ) -> Self {
        // Number the regions the constraints of the body mention.
        let mut numbering = RegionNumbering::default();
        let mut constraint_edges = Vec::new();
        for (lesser, point, greater) in constraints.outlives() {
            let (lesser, greater) = (numbering.index_of(lesser), numbering.index_of(greater));
            constraint_edges.push((lesser, Edge { point, greater }));
        }

        // The universal regions the environment relates are part of the
        // graph even when no constraint mentions them: a where clause may be
        // about a lifetime the body never uses.
        for lifetime in environment.lifetimes() {
            numbering.index_of(lifetime);
        }

        // Every region is numbered from here on.
        let RegionNumbering { regions, indices } = numbering;
        let size = regions.len();

        let universals: Vec<_> =
            (0..size).filter(|&index| regions[index].is_universal_region()).collect();
        let static_region = universals
            .iter()
            .copied()
            .find(|&universal| regions[universal].is_static_lifetime())
            .expect("the environment always mentions `'static`");

        // What the body requires is the closure of its constraints.
        let mut pairs: Vec<_> =
            constraint_edges.iter().map(|&(lesser, edge)| (lesser, edge.greater)).collect();
        let required = TransitiveClosure::new(pairs.iter().copied(), size, true)
            .expect("every edge refers to an indexed region");

        // A universal region also outlives what the environment states.
        for &longer in &universals {
            for &shorter in &universals {
                if longer != shorter
                    && environment.region_outlives(&regions[longer], &regions[shorter])
                {
                    pairs.push((longer, shorter));
                }
            }
        }
        let implied = TransitiveClosure::new(pairs.iter().copied(), size, true)
            .expect("every edge refers to an indexed region");

        // Group the constraints by the region they are out of, for the
        // search that explains an error.
        let mut edges = vec![Vec::new(); size];
        for (lesser, edge) in constraint_edges {
            edges[lesser].push(edge);
        }

        Self { regions, indices, edges, universals, static_region, required, implied }
    }

    /// Iterates over the universal regions of the graph: the ones with a
    /// constraint into or out of them, and the ones the environment
    /// mentions.
    pub(crate) fn universals(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.universals.iter().map(|&universal| &self.regions[universal])
    }

    /// Iterates over the universal regions other than `region` that the
    /// constraints of the body require it to outlive, through any number of
    /// regions, universal ones included.
    pub(crate) fn reachable_universals<'s>(
        &'s self,
        region: &Interned<Ty>,
    ) -> impl Iterator<Item = &'s Interned<Ty>> {
        let index = self.indices.get(region).copied();

        index.into_iter().flat_map(move |index| {
            self.universals
                .iter()
                .filter(move |&&universal| {
                    universal != index
                        && self.required.has_path(index, universal).expect("both are indexed")
                })
                .map(|&universal| &self.regions[universal])
        })
    }

    /// Searches the graph breadth-first from `source`, through the regions of
    /// the body, up to each universal region, recording the constraints it
    /// follows.
    ///
    /// The search stops at each universal region it reaches: what that
    /// region must outlive in turn is found by the search from it.
    ///
    /// This search is indeed expensive, but it is only done when there is an
    /// error to report
    pub(crate) fn reach_universals_from(&self, source: &Interned<Ty>) -> Reached<'_> {
        let mut reached =
            Reached { graph: self, universals: Vec::new(), parents: FxHashMap::default() };
        let Some(&source) = self.indices.get(source) else {
            return reached;
        };

        let mut pending = VecDeque::from_iter([source]);
        while let Some(region) = pending.pop_front() {
            for edge in &self.edges[region] {
                let greater = edge.greater;
                if greater == source || reached.parents.contains_key(&greater) {
                    continue;
                }
                reached.parents.insert(greater, (region, edge.point));

                if self.regions[greater].is_universal_region() {
                    reached.universals.push(greater);
                } else {
                    pending.push_back(greater);
                }
            }
        }

        reached
    }

    /// Returns whether `'lesser: 'greater` follows from the constraints of
    /// the body together with the relations the environment states between
    /// universal regions.
    ///
    /// When it follows in both directions, the two regions are the same
    /// lifetime in every solution of the constraints.
    pub(crate) fn implies_outlives(&self, lesser: &Interned<Ty>, greater: &Interned<Ty>) -> bool {
        if lesser == greater {
            return true;
        }

        // A region that is not in the graph has no constraint, and is not
        // mentioned by the environment: nothing says what it outlives.
        let Some(&lesser) = self.indices.get(lesser) else {
            return false;
        };

        // Likewise, nothing says what outlives such a region, except that a
        // region outliving `'static` outlives every universal region.
        let greater = match self.indices.get(greater) {
            Some(&greater) => greater,
            None if greater.is_universal_region() => self.static_region,
            None => return false,
        };

        self.implied.has_path(lesser, greater).expect("both are indexed")
    }

    /// Returns a universal region that the constraints of the body and the
    /// environment make the same lifetime as `region`, if there is one:
    /// the least of them, so that the choice is stable.
    pub(crate) fn equal_universal(&self, region: &Interned<Ty>) -> Option<&Interned<Ty>> {
        self.universals()
            .filter(|universal| {
                self.implies_outlives(region, universal) && self.implies_outlives(universal, region)
            })
            .min()
    }
}

impl RegionRelation for SubsetGraph {
    fn outlives(&self, lesser: &Interned<Ty>, greater: &Interned<Ty>) -> bool {
        self.implies_outlives(lesser, greater)
    }
}

/// The universal regions a search reached from its source, and how.
pub(crate) struct Reached<'a> {
    graph: &'a SubsetGraph,

    /// The universal regions reached, in the order the search found them.
    universals: Vec<usize>,

    /// For each region reached, the region it was reached from and the point
    /// of the constraint between them.
    parents: FxHashMap<usize, (usize, Point)>,
}

impl<'a> Reached<'a> {
    /// Iterates over the universal regions reached, in the order the search
    /// found them.
    pub(crate) fn universals(&self) -> impl Iterator<Item = &'a Interned<Ty>> + '_ {
        let graph = self.graph;
        self.universals.iter().map(move |&universal| &graph.regions[universal])
    }

    /// Returns the points of the constraints on the path the search took to
    /// `region`, from the source of the search onwards. There is none when
    /// the search did not reach `region`.
    pub(crate) fn path_points(&self, region: &Interned<Ty>) -> Vec<Point> {
        let mut points = Vec::new();
        let mut current = self.graph.indices.get(region).copied();
        while let Some(&(parent, point)) = current.and_then(|current| self.parents.get(&current)) {
            points.push(point);
            current = Some(parent);
        }

        points.reverse();
        points
    }
}
