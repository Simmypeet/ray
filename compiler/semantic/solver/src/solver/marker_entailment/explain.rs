//! The witness of a marker goal that does not hold: the shortest chain of
//! premises from the goal to one that fails for a reason of its own.
//!
//! Marker entailment records only its answers, so that the many goals that
//! hold cost nothing more. The witness is searched for after a goal is known
//! to fail, with the same rules evaluation used.

use rayc_hash::FxHashSet;
use rayc_type::where_clause::{MarkerPredicate, PredicateKind};

use super::MarkerRule;
use crate::{
    Solver,
    explain::{Goal, Leaf, ProofFailure, Reason, Step, StructuralComponent},
};

/// The most goals the search visits before it gives up. A type with
/// polymorphic recursion has a new premise at every level.
const MAX_VISITED_GOALS: usize = 256;

/// A goal the search reached, with the goal it was reached from, by index,
/// and the step between them.
struct Visited {
    goal: MarkerPredicate,
    parent: Option<(usize, Step)>,
}

impl Solver {
    /// Explains why `predicate` does not hold, by finding the shortest chain
    /// of premises from it to a goal that fails for a reason of its own.
    ///
    /// Returns `None` when the predicate holds.
    ///
    /// # Panics
    ///
    /// If a marker goal is being proved: every premise is evaluated as a
    /// root goal, so that its answer is its own fixpoint.
    pub async fn explain_marker(&mut self, predicate: MarkerPredicate) -> Option<ProofFailure> {
        assert!(
            !self.marker_entailment.has_active_goal(),
            "a marker goal is explained between root goals"
        );

        let root = self.normalize_marker_goal(predicate).await;
        let answer = self.evaluate_marker_goal(root.clone()).await;
        if answer.holds() {
            return None;
        }

        // Search breadth first, so the first goal that fails on its own ends
        // the shortest chain. A goal reached again was already reached on a
        // chain no longer than this one, so one set of seen goals is enough,
        // and the search never follows a cycle.
        let mut visited = vec![Visited { goal: root.clone(), parent: None }];
        let mut seen = FxHashSet::from_iter([root]);
        let mut next = 0;
        while next < visited.len() && next < MAX_VISITED_GOALS {
            let goal = visited[next].goal.clone();
            let premises = match self.marker_rule(&goal).await {
                MarkerRule::BuiltIn(rule) => {
                    return Some(witness(&visited, next, Leaf::BuiltIn(rule)));
                }
                MarkerRule::Negative(implementation_id) => {
                    return Some(witness(
                        &visited,
                        next,
                        Leaf::NegativeImplementation(implementation_id),
                    ));
                }
                MarkerRule::Opaque => return Some(witness(&visited, next, Leaf::Opaque)),
                MarkerRule::Holds => Vec::new(),
                MarkerRule::Explicit { implementation_id, premises } => {
                    // An equality premise is rejected where the
                    // implementation is declared, and always fails here.
                    if premises.iter().any(|premise| premise.as_equality().is_some()) {
                        return Some(witness(&visited, next, Leaf::Unexplained));
                    }

                    premises
                        .into_iter()
                        .filter_map(|premise| match premise {
                            PredicateKind::Marker(premise) => Some((
                                Step::MarkerImplementationPremise { implementation_id },
                                premise,
                            )),

                            // An outlives premise always holds.
                            PredicateKind::Outlives(_)
                            | PredicateKind::AssociatedTypeEquality(_) => None,
                        })
                        .collect()
                }
                MarkerRule::Structural(premises) => premises
                    .into_iter()
                    .map(|(component, premise)| (Step::Structural(component), premise))
                    .collect::<Vec<_>>(),
            };

            // Follow only the premises with the same answer as the root:
            // one of them is why this goal has it.
            for (step, premise) in premises {
                let premise = self.normalize_marker_goal(premise).await;
                if !seen.insert(premise.clone()) {
                    continue;
                }
                if self.evaluate_marker_goal(premise.clone()).await == answer {
                    visited.push(Visited { goal: premise, parent: Some((next, step)) });
                }
            }

            next += 1;
        }

        // Every goal that fails has a premise that fails, or fails on its
        // own, so only the budget ends the search without a leaf.
        debug_assert!(next == MAX_VISITED_GOALS, "a failing marker goal must have a witness");
        Some(witness(&visited, 0, Leaf::Unexplained))
    }
}

/// Builds the chain from the root goal to the visited goal at `end`, which
/// fails for `leaf`.
fn witness(visited: &[Visited], end: usize, leaf: Leaf) -> ProofFailure {
    // Collect the chain from its end up to the root, each goal with the step
    // into it.
    let mut chain = Vec::new();
    let mut index = Some(end);
    while let Some(current) = index {
        let Visited { goal, parent } = &visited[current];
        chain.push((goal.clone(), parent.map(|(_, step)| step)));
        index = parent.map(|(parent, _)| parent);
    }
    chain.reverse();

    // A closure stores the tuple of its captures, which is not written in
    // source. When the tuple is structural too, the chain goes from the
    // closure to the capture directly.
    let mut index = 1;
    while index + 1 < chain.len() {
        if let (
            Some(Step::Structural(StructuralComponent::ClosureCaptures)),
            Some(Step::Structural(StructuralComponent::TupleElement(capture))),
        ) = (chain[index].1, chain[index + 1].1)
        {
            chain.remove(index);
            chain[index].1 = Some(Step::Structural(StructuralComponent::ClosureCapture(capture)));
        }
        index += 1;
    }

    // Nest the failures from the leaf up, each goal requiring the next.
    let mut chain = chain.into_iter().rev();
    let (goal, mut step) = chain.next().expect("the chain has at least the root goal");
    let mut failure = ProofFailure::new(Goal::Marker(goal), Reason::Leaf(leaf));
    for (goal, parent_step) in chain {
        let step_into = step.expect("only the root goal has no step into it");
        failure = ProofFailure::new(Goal::Marker(goal), Reason::Requires {
            step: step_into,
            failure: Box::new(failure),
        });
        step = parent_step;
    }

    failure
}
