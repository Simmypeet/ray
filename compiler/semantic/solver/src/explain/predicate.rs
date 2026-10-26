//! Explanations of where-clause predicates that do not hold.

use rayc_type::where_clause::PredicateKind;

use super::ProofFailure;
use crate::Solver;

impl Solver {
    /// Explains why `predicate` does not hold at the site.
    ///
    /// Returns `None` for a predicate that no proof explains, which keeps the
    /// report it has today: an equality, an outlives predicate, or a marker
    /// goal that holds. A marker goal is not explained either when it has
    /// inference variables, which make it fail because it is not known yet,
    /// or when it involves the error type, whose diagnostic was already
    /// reported.
    pub async fn explain_predicate(&mut self, predicate: &PredicateKind) -> Option<ProofFailure> {
        match predicate {
            PredicateKind::Marker(marker) => {
                let implementor = marker.implementor();
                if implementor.contains_non_lifetime_inference() || implementor.contains_error() {
                    return None;
                }
                self.explain_marker(marker.clone()).await
            }
            PredicateKind::AssociatedTypeEquality(_) | PredicateKind::Outlives(_) => None,
        }
    }
}
