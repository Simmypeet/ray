//! The location-insensitive constraint graph of an IR function: for each
//! region, the regions it must outlive somewhere in the body. It answers what
//! the body requires of universal regions, which are live at every point.

use std::collections::VecDeque;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{cfg::Point, ir_function::IRFunction};
use rayc_lexical::tree::RelativeSpan;
use rayc_solver::outlives::{OutlivesEnvironment, RegionRelation};
use rayc_transitive_closure::TransitiveClosure;
use rayc_type::ty::Ty;

use crate::constraint::LocalizedConstraints;

/// A constraint `'lesser: 'greater` out of a region `'lesser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Edge {
    /// The point of the instruction requiring the constraint.
    point: Point,

    /// The source requiring the constraint, when a nested function created at
    /// `point` does.
    blame: Option<RelativeSpan>,

    /// The index of `'greater` in the graph.
    greater: usize,
}

impl Edge {
    /// Returns the source requiring the constraint in `function`, if it has
    /// one.
    fn span(&self, function: &IRFunction) -> Option<RelativeSpan> {
        self.blame.or_else(|| function.point_span(self.point))
    }
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
    /// The regions of the graph, each referred to by its index here.
    regions: Vec<Interned<Ty>>,

    /// The index of each region in `regions`.
    indices: FxHashMap<Interned<Ty>, usize>,

    /// The constraints out of each region.
    edges: Vec<Vec<Edge>>,

    /// The indices of the universal regions.
    universals: Vec<usize>,

    /// The index of `'static`.
    static_region: usize,

    /// Has a path from `a` to `b` when the constraints of the body require `a:
    /// b`.
    required: TransitiveClosure,

    /// Has a path from `a` to `b` when `a: b` follows from the constraints of
    /// the body and the environment.
    implied: TransitiveClosure,
}

impl SubsetGraph {
    /// Builds the graph of the union of `constraints` over every point.
    pub(crate) fn new(
        constraints: &LocalizedConstraints,
        environment: &OutlivesEnvironment,
    ) -> Self {
        // Number the regions the constraints of the body mention.
        let mut numbering = RegionNumbering::default();
        let mut constraint_edges = Vec::new();
        for constraint in constraints.outlives() {
            let lesser = numbering.index_of(constraint.lesser());
            let greater = numbering.index_of(constraint.greater());
            let edge = Edge { point: constraint.point(), blame: constraint.blame(), greater };
            constraint_edges.push((lesser, edge));
        }

        // A where clause may be about a lifetime the body never uses.
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

    /// Iterates over the universal regions of the graph.
    pub(crate) fn universals(&self) -> impl Iterator<Item = &Interned<Ty>> {
        self.universals.iter().map(|&universal| &self.regions[universal])
    }

    /// Iterates over the universal regions other than `region` that the
    /// constraints of the body require it to outlive.
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

    /// Searches breadth-first from `source` up to each universal region,
    /// recording the constraints followed. Only done to explain an error.
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
                reached.parents.insert(greater, (region, *edge));

                if self.regions[greater].is_universal_region() {
                    reached.universals.push(greater);
                } else {
                    pending.push_back(greater);
                }
            }
        }

        reached
    }

    /// Returns whether `'lesser: 'greater` follows from the constraints of the
    /// body and the environment.
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

    /// Returns the least universal region that the constraints and the
    /// environment make the same lifetime as `region`, if there is one.
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

    /// For each region reached, the region it was reached from and the
    /// constraint between them.
    parents: FxHashMap<usize, (usize, Edge)>,
}

impl<'a> Reached<'a> {
    /// Iterates over the universal regions reached, in the order found.
    pub(crate) fn universals(&self) -> impl Iterator<Item = &'a Interned<Ty>> + '_ {
        let graph = self.graph;
        self.universals.iter().map(move |&universal| &graph.regions[universal])
    }

    /// Returns the source of each constraint on the path the search took to
    /// `region`, in order. The path is empty when `region` was not reached.
    pub(crate) fn path_spans(
        &self,
        region: &Interned<Ty>,
        function: &IRFunction,
    ) -> Vec<Option<RelativeSpan>> {
        let mut spans = Vec::new();
        let mut current = self.graph.indices.get(region).copied();
        while let Some((parent, edge)) = current.and_then(|current| self.parents.get(&current)) {
            spans.push(edge.span(function));
            current = Some(*parent);
        }

        spans.reverse();
        spans
    }
}
