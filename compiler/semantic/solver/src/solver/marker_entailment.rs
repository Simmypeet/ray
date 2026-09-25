//! Coinductive entailment for marker predicates.
//!
//! Marker implementations act as constructor rules, while types without an
//! explicit rule receive the same structural treatment as Rust auto traits.

use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{
    all_marker_implementations::get_all_marker_implementations,
    marker_implementation::get_marker_implementation, struct_body::get_struct_body,
};
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{
    poly_var::build_subst_from_args,
    subst::Substitutable,
    ty::{Mutability, Ty, application::View as ApplicationView},
    where_clause::{MarkerPredicate, PredicateKind, get_where_clause},
};

use crate::Solver;

#[derive(Debug)]
struct MarkerEntailmentFrame {
    goal: MarkerPredicate,
    memo: bool,
    provisional_result: Option<bool>,
}

#[derive(Debug)]
struct ActiveMarkerGoal {
    depth: usize,
    goal: MarkerPredicate,
}

#[derive(Debug)]
enum EnteredMarkerGoal {
    Memoized(bool),
    Provisional(bool),
    Active(ActiveMarkerGoal),
}

enum ExplicitMarkerRule {
    Positive(Vec<PredicateKind>),
    Negative,
}

/// State shared by marker goals solved with the same givens.
#[derive(Debug, Default)]
pub(crate) struct MarkerEntailmentState {
    active_goals: Vec<MarkerEntailmentFrame>,
    memo: FxHashMap<MarkerPredicate, bool>,
}

impl MarkerEntailmentState {
    fn enter_goal(&mut self, goal: MarkerPredicate) -> EnteredMarkerGoal {
        // Reuse only results that completed outside an unstable cyclic path.
        if let Some(result) = self.memo.get(&goal).copied() {
            return EnteredMarkerGoal::Memoized(result);
        }

        // A repeated active goal reads the cyclic root's provisional value,
        // initially true for a coinductive proof. Descendants depend on that
        // provisional value and therefore cannot be memoized independently.
        if let Some(cycle_start) = self.active_goals.iter().position(|frame| frame.goal == goal) {
            let result = *self.active_goals[cycle_start].provisional_result.get_or_insert(true);
            for frame in &mut self.active_goals[cycle_start + 1..] {
                frame.memo = false;
            }
            return EnteredMarkerGoal::Provisional(result);
        }

        let active = ActiveMarkerGoal { depth: self.active_goals.len(), goal: goal.clone() };
        self.active_goals.push(MarkerEntailmentFrame {
            goal,
            memo: true,
            provisional_result: None,
        });
        EnteredMarkerGoal::Active(active)
    }

    /// Updates a changed provisional value while keeping its frame active for
    /// another evaluation of the same goal.
    fn update_provisional_result(&mut self, active: &ActiveMarkerGoal, result: bool) -> bool {
        let ActiveMarkerGoal { depth, goal } = active;
        assert_eq!(depth + 1, self.active_goals.len(), "marker goals must leave in stack order");
        let frame = &mut self.active_goals[*depth];
        assert_eq!(&frame.goal, goal, "the active marker goal token must match the stack");

        if let Some(provisional) = frame.provisional_result
            && provisional != result
        {
            frame.provisional_result = Some(result);
            true
        } else {
            false
        }
    }

    fn complete_goal(&mut self, active: ActiveMarkerGoal, result: bool) {
        let ActiveMarkerGoal { depth, goal } = active;
        assert_eq!(depth + 1, self.active_goals.len(), "marker goals must leave in stack order");
        let frame = self.active_goals.pop().expect("an active marker goal must be present");
        assert_eq!(frame.goal, goal, "the active marker goal token must match the stack");
        if frame.memo {
            self.memo.insert(goal, result);
        }
    }
}

impl Solver {
    /// Returns whether a marker predicate follows from visible givens,
    /// explicit marker rules, or structural auto-trait inference.
    pub async fn entails_marker_predicate(&mut self, predicate: MarkerPredicate) -> bool {
        let implementor = self.normalize(predicate.implementor()).await;
        let goal = MarkerPredicate::new(predicate.marker_id(), implementor);
        self.evaluate_marker_goal(goal).await
    }

    async fn evaluate_marker_goal(&mut self, goal: MarkerPredicate) -> bool {
        let implementor = self.normalize(goal.implementor()).await;
        let goal = MarkerPredicate::new(goal.marker_id(), implementor);
        let active = match self.marker_entailment.enter_goal(goal.clone()) {
            EnteredMarkerGoal::Memoized(result) | EnteredMarkerGoal::Provisional(result) => {
                return result;
            }
            EnteredMarkerGoal::Active(active) => active,
        };

        // Keep the cyclic root on the stack while revising its provisional
        // result. It is complete only when one evaluation reproduces that value.
        loop {
            let result = Box::pin(self.prove_active_marker_goal(&goal)).await;
            if self.marker_entailment.update_provisional_result(&active, result) {
                // try again until fixpoint is reached
                continue;
            }

            self.marker_entailment.complete_goal(active, result);
            return result;
        }
    }

    async fn prove_active_marker_goal(&mut self, goal: &MarkerPredicate) -> bool {
        // `Copy` for references is built in: a shared reference is always
        // `Copy`, and a unique one never is.
        if let Some(reference) = goal.implementor().as_reference_view()
            && goal.marker_id() == self.engine().get_core_item(CoreItem::Copy).await
        {
            return reference.mutability() == Mutability::Immutable;
        }

        // A matching visible predicate is a leaf proof. Equality is checked
        // without allowing entailment to bind either side.
        let matching_givens = self
            .givens()
            .iter()
            .filter_map(|predicate| match predicate {
                PredicateKind::Marker(predicate) if predicate.marker_id() == goal.marker_id() => {
                    Some(predicate.implementor().clone())
                }
                PredicateKind::AssociatedTypeEquality(_) | PredicateKind::Marker(_) => None,
            })
            // we have to collect here because the `eq_without_unify` call below mutably borrows
            // `self`, which prevents us from using the iterator directly
            .collect::<Vec<_>>();

        for given in matching_givens {
            if self.eq_without_unify(&given, goal.implementor()).await {
                return true;
            }
        }

        // An explicit implementation replaces the synthesized structural rule
        // for its constructor and contributes its substituted marker premises.
        if let Some(rule) = self.explicit_marker_rule(goal).await {
            match rule {
                ExplicitMarkerRule::Positive(predicates) => {
                    let mut entailed = true;
                    for predicate in predicates {
                        let premise = match predicate {
                            PredicateKind::Marker(predicate) => {
                                self.evaluate_marker_goal(predicate).await
                            }

                            // shouldn't happen because marker implementation predicates can only
                            // be marker predicates
                            PredicateKind::AssociatedTypeEquality(_) => false,
                        };
                        entailed &= premise;
                    }
                    return entailed;
                }
                ExplicitMarkerRule::Negative => return false,
            }
        }

        // Without an explicit rule, synthesize the auto-trait rule from the
        // values stored by the type. Empty structural types prove immediately.
        let Some(fields) = structural_fields(goal.implementor(), self.engine()).await else {
            return false;
        };
        let mut entailed = true;
        for field in fields {
            let predicate = MarkerPredicate::new(goal.marker_id(), field);
            entailed &= self.evaluate_marker_goal(predicate).await;
        }
        entailed
    }

    async fn explicit_marker_rule(&mut self, goal: &MarkerPredicate) -> Option<ExplicitMarkerRule> {
        let engine = self.engine().clone();
        let target_id = self.site().target_id;
        let implementations =
            engine.get_all_marker_implementations(goal.marker_id(), target_id).await;

        for symbol_id in implementations.iter().copied() {
            let implementation = engine.get_marker_implementation(symbol_id).await;

            let Some(subst) = self
                .type_head_match(implementation.implementor().clone(), goal.implementor().clone())
                .await
            else {
                continue;
            };

            if implementation.is_negative() {
                return Some(ExplicitMarkerRule::Negative);
            }

            let clause = engine.get_where_clause(symbol_id).await;

            return Some(ExplicitMarkerRule::Positive(
                clause
                    .iter()
                    .map(|predicate| predicate.kind().apply_subst_or_clone(&subst, self.engine()))
                    .collect(),
            ));
        }

        None
    }
}

/// Returns the values whose marker properties determine the enclosing type.
/// `None` represents an opaque or ill-kinded type that cannot be inferred.
async fn structural_fields(
    ty: &Ty,
    engine: &TrackedEngine,
) -> Option<Vec<qbice::storage::intern::Interned<Ty>>> {
    match ty {
        Ty::Application(application) => match application.view() {
            ApplicationView::Primitive(_) => Some(Vec::new()),
            ApplicationView::Tuple(tuple) => Some(tuple.args().to_vec()),
            ApplicationView::Closure(closure) => Some(vec![closure.captured_tuple().clone()]),
            ApplicationView::Struct(struct_) => {
                let body = engine.get_struct_body(struct_.symbol_id()).await;
                let subst = engine.build_subst_from_args(struct_.symbol_id(), struct_.args()).await;

                Some(
                    body.iter()
                        .map(|(_, field)| field.ty().apply_subst_or_clone(&subst, engine))
                        .collect(),
                )
            }
            ApplicationView::Pointer(_)
            | ApplicationView::Reference(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::Error => None,
        },
        Ty::Inference(_)
        | Ty::PolyVar(_)
        | Ty::SelfInstance(_)
        | Ty::EffectRow(_)
        | Ty::Lifetime(_) => None,
    }
}
