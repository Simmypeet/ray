//! The points at which each loan of an IR function is live: where it flows,
//! through the localized constraint graph, into a region that is live there.

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{cfg::Point, ir_function::IRFunction};
use rayc_type::{ty::Ty, variance::Variance};

use crate::{
    constraint::{Loan, LoanID, LocalizedConstraints},
    region_liveness::RegionLiveness,
    variance::LifetimeVariances,
};

/// The points at which each loan of an IR function is live.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveLoans {
    // TODO: We'd need a more compact representation in the future. In rustc, they use a matrix of
    // bitsets
    /// The loans live at each point. A point where no loan is live has no
    /// entry.
    loans_by_point: FxHashMap<Point, FxHashSet<LoanID>>,
}

impl LiveLoans {
    /// Computes the points at which each loan of `constraints` is live.
    #[must_use]
    pub fn compute(
        function: &IRFunction,
        constraints: &LocalizedConstraints,
        liveness: &RegionLiveness,
        variances: &LifetimeVariances,
    ) -> Self {
        let traversal = Traversal::new(function, constraints, liveness, variances);
        let mut live_loans = Self::default();

        for (loan_id, loan) in constraints.loans() {
            for point in traversal.live_points(loan) {
                live_loans.loans_by_point.entry(point).or_default().insert(loan_id);
            }
        }

        live_loans
    }

    /// Returns whether `loan` is live at `point`.
    #[must_use]
    pub fn is_live(&self, loan: LoanID, point: Point) -> bool {
        self.loans_by_point.get(&point).is_some_and(|loans| loans.contains(&loan))
    }
}

/// A region at a point: a node of the localized constraint graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Node {
    region: Interned<Ty>,
    point: Point,
}

/// Which ways the liveness edges of a region go.
#[derive(Debug, Clone, Copy)]
struct LivenessEdges {
    forward: bool,
    backward: bool,
}

impl LivenessEdges {
    /// Returns the liveness edges of a region with `variance`. A bivariant
    /// region is never related to another, so it needs none.
    const fn of_variance(variance: Variance) -> Self {
        match variance {
            Variance::Covariant => Self { forward: true, backward: false },
            Variance::Contravariant => Self { forward: false, backward: true },
            Variance::Invariant => Self { forward: true, backward: true },
            Variance::Bivariant => Self { forward: false, backward: false },
        }
    }
}

/// The search of the localized constraint graph for where each loan flows.
pub(crate) struct Traversal<'a> {
    function: &'a IRFunction,
    constraints: &'a LocalizedConstraints,
    liveness: &'a RegionLiveness,
    variances: &'a LifetimeVariances,
}

impl<'a> Traversal<'a> {
    /// Creates the search over the localized constraint graph of `function`.
    pub(crate) const fn new(
        function: &'a IRFunction,
        constraints: &'a LocalizedConstraints,
        liveness: &'a RegionLiveness,
        variances: &'a LifetimeVariances,
    ) -> Self {
        Self { function, constraints, liveness, variances }
    }

    /// Returns every point at which `loan` flows into a live region.
    fn live_points(&self, loan: &Loan) -> impl Iterator<Item = Point> {
        self.reached(loan)
            .into_iter()
            .filter(|node| self.liveness.is_live(&node.region, node.point))
            .map(|node| node.point)
    }

    /// Returns the regions that hold `loan` at `point` and are live there. This
    /// searches the graph again, so it is only for reporting an error.
    pub(crate) fn regions_holding(
        &self,
        loan: &Loan,
        point: Point,
    ) -> impl Iterator<Item = Interned<Ty>> + '_ {
        self.reached(loan)
            .into_iter()
            .filter(move |node| node.point == point && self.liveness.is_live(&node.region, point))
            .map(|node| node.region)
    }

    /// Returns every node that `loan` reaches from the node of its own region
    /// at the point of its borrow.
    fn reached(&self, loan: &Loan) -> FxHashSet<Node> {
        let start = Node { region: loan.region().clone(), point: loan.point() };
        let mut visited = FxHashSet::from_iter([start.clone()]);
        let mut stack = vec![start];

        while let Some(node) = stack.pop() {
            let is_live = self.liveness.is_live(&node.region, node.point);

            let mut visit = |next: Node| {
                if visited.insert(next.clone()) {
                    stack.push(next);
                }
            };

            // The regions `node.region` flows into at this point.
            for greater in self.constraints.outlived_regions(&node.region, node.point) {
                visit(Node { region: greater.clone(), point: node.point });
            }

            let edges = self.liveness_edges(&node.region);

            // The region keeps its loans as control moves on, for as long as
            // it stays live.
            if edges.forward {
                for successor in self.function.successor_points(node.point) {
                    if self.liveness.is_live(&node.region, successor) {
                        visit(Node { region: node.region.clone(), point: successor });
                    }
                }
            }

            // What flowed into a live contravariant region earlier may use
            // the loans it holds now.
            if edges.backward && is_live {
                for predecessor in self.function.predecessor_points(node.point) {
                    visit(Node { region: node.region.clone(), point: predecessor });
                }
            }
        }

        visited
    }

    /// Returns the liveness edges of `region`.
    fn liveness_edges(&self, region: &Interned<Ty>) -> LivenessEdges {
        // A universal region is live everywhere, so forward edges alone reach
        // every point after the loan enters it.
        if region.is_universal_region() {
            return LivenessEdges::of_variance(Variance::Covariant);
        }

        let variance =
            self.variances.get(region).expect("every region should occur in a type of the IR");
        LivenessEdges::of_variance(variance)
    }
}

#[cfg(test)]
mod tests;
