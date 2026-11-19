//! Explanations of where-clause predicates that do not hold.

use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::where_clause::PredicateKind;

use super::{ExplainedGoal, ProofFailure, RequiredBy, UnexplainedPredicate, UnsatisfiedPredicate};
use crate::Solver;

impl Solver {
    /// Reports that `predicate` from the where clause of `symbol_id` does not
    /// hold at `span`, with its failing premises when they can be explained.
    pub async fn explain_unsatisfied_predicate(
        &mut self,
        predicate: PredicateKind,
        symbol_id: GlobalSymbolID,
        span: RelativeSpan,
    ) -> UnsatisfiedPredicate {
        let failure = self.explain_predicate(&predicate).await;
        failure.map_or_else(
            || {
                UnsatisfiedPredicate::Unexplained(UnexplainedPredicate::new(
                    span, symbol_id, predicate,
                ))
            },
            |failure| {
                UnsatisfiedPredicate::Explained(Box::new(ExplainedGoal::new(
                    span,
                    RequiredBy::WhereClause { symbol_id },
                    failure,
                )))
            },
        )
    }

    /// Explains why `predicate` does not hold, or `None` when no proof can
    /// explain it or the failure is already reported.
    async fn explain_predicate(&mut self, predicate: &PredicateKind) -> Option<ProofFailure> {
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
