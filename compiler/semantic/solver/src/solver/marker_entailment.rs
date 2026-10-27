//! Coinductive entailment for marker predicates.
//!
//! Marker implementations act as constructor rules, while types without an
//! explicit rule receive the same structural treatment as Rust auto traits.

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::get_ir;
use rayc_semantic_element::{
    all_marker_implementations::get_all_marker_implementations,
    marker_implementation::get_marker_implementation,
    parameter::get_parameter_map,
    saved_local::{ClosureCoroutineBodyID, CoroutineBody, get_saved_locals},
    struct_body::get_struct_body,
    variant_payload_ty::get_variant_payload_ty,
};
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
    member::get_members,
    syntax::is_linear_type,
};
use rayc_type::{
    poly_var::build_subst_from_args,
    subst::Substitutable,
    ty::{Mutability, Ty, application::View as ApplicationView},
    where_clause::{MarkerPredicate, PredicateKind, get_where_clause},
};

use crate::{
    Solver,
    explain::{BuiltInMarkerRule, CoroutineSavedLocal, StructuralComponent},
};

mod explain;

/// The answer to a marker goal.
///
/// The answers are ordered from the weakest to the strongest. A goal whose
/// premises must all hold takes the weakest answer among them, so one premise
/// that fails makes the goal fail even when another is deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MarkerEntailment {
    /// The goal does not follow.
    Fails,

    /// The goal could not be decided within the solver's depth limit.
    Overflow,

    /// The goal depends on what a coroutine holds, which is not visible to
    /// this solver. It is proved later, by a solver that can see it.
    Deferred,

    /// The goal follows.
    Holds,
}

impl MarkerEntailment {
    /// Returns whether the goal follows.
    #[must_use]
    pub const fn holds(self) -> bool { matches!(self, Self::Holds) }

    /// Returns the answer for a goal that follows exactly when `holds` is
    /// true.
    #[must_use]
    pub const fn from_bool(holds: bool) -> Self { if holds { Self::Holds } else { Self::Fails } }
}

#[derive(Debug)]
struct MarkerEntailmentFrame {
    goal: MarkerPredicate,
    memo: bool,
    provisional_result: Option<MarkerEntailment>,
}

#[derive(Debug)]
struct ActiveMarkerGoal {
    depth: usize,
    goal: MarkerPredicate,
}

#[derive(Debug)]
enum EnteredMarkerGoal {
    Memoized(MarkerEntailment),
    Provisional(MarkerEntailment),
    Active(ActiveMarkerGoal),
}

/// The rule that decides a marker goal, with its premises not yet
/// evaluated.
#[derive(Debug)]
enum MarkerRule {
    /// The goal holds without premises: a given provides it, or it is `Copy`
    /// of a shared reference.
    Holds,

    /// The compiler rejects the goal, whatever the givens say.
    BuiltIn(BuiltInMarkerRule),

    /// An explicit marker implementation applies, and the goal holds when
    /// its premises do.
    Explicit { implementation_id: GlobalSymbolID, premises: Vec<PredicateKind> },

    /// A negative marker implementation excludes the type.
    Negative(GlobalSymbolID),

    /// No explicit implementation applies, and the goal holds when every
    /// value the type stores satisfies the marker.
    Structural(Vec<(StructuralComponent, MarkerPredicate)>),

    /// No rule applies to the type.
    Opaque,

    /// [`MarkerEntailmentState`]'s is told to defer evaluating the saved locals
    /// of the coroutine, which could cause cyclic queries.
    CoroutineDeferred,
}

/// Determines whether when encoutering a coroutine, should the solver query
/// for its saved locals or defer the query to be solved later when ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CoroutineSavedLocalsVisibility {
    /// The solver is allowed to query for the saved locals of the coroutine.
    #[default]
    Visible,

    /// The solver shouldn't query for the saved locals of the coroutine.
    Deferred,
}

/// State shared by marker goals solved with the same givens.
#[derive(Debug, Default)]
pub(crate) struct MarkerEntailmentState {
    active_goals: Vec<MarkerEntailmentFrame>,
    memo: FxHashMap<MarkerPredicate, MarkerEntailment>,
    visibility: CoroutineSavedLocalsVisibility,
}

impl MarkerEntailmentState {
    #[must_use]
    pub fn new(visibility: CoroutineSavedLocalsVisibility) -> Self {
        Self { active_goals: Vec::new(), memo: FxHashMap::default(), visibility }
    }

    /// Returns whether a marker goal is being proved.
    const fn has_active_goal(&self) -> bool { !self.active_goals.is_empty() }

    fn enter_goal(&mut self, goal: MarkerPredicate) -> EnteredMarkerGoal {
        // Reuse only results that completed outside an unstable cyclic path.
        if let Some(result) = self.memo.get(&goal).copied() {
            return EnteredMarkerGoal::Memoized(result);
        }

        // A repeated active goal reads the cyclic root's provisional value,
        // initially `Holds` for a coinductive proof. Each revision can only
        // lower it, so the fixpoint loop ends. Descendants depend on that
        // provisional value and therefore cannot be memoized independently.
        if let Some(cycle_start) = self.active_goals.iter().position(|frame| frame.goal == goal) {
            let result = *self.active_goals[cycle_start]
                .provisional_result
                .get_or_insert(MarkerEntailment::Holds);
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
    fn update_provisional_result(
        &mut self,
        active: &ActiveMarkerGoal,
        result: MarkerEntailment,
    ) -> bool {
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

    fn complete_goal(&mut self, active: ActiveMarkerGoal, result: MarkerEntailment) {
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
    pub async fn entails_marker_predicate(
        &mut self,
        predicate: MarkerPredicate,
    ) -> MarkerEntailment {
        let goal = self.normalize_marker_goal(predicate).await;
        self.evaluate_marker_goal(goal).await
    }

    /// Returns `goal` with its implementor normalized, which is the form the
    /// goal is memoized and its rule selected in.
    async fn normalize_marker_goal(&self, goal: MarkerPredicate) -> MarkerPredicate {
        let implementor = self.normalize(goal.implementor()).await;
        MarkerPredicate::new(goal.marker_id(), implementor)
    }

    async fn evaluate_marker_goal(&mut self, goal: MarkerPredicate) -> MarkerEntailment {
        let goal = self.normalize_marker_goal(goal).await;
        let active = match self.marker_entailment.enter_goal(goal.clone()) {
            EnteredMarkerGoal::Memoized(result) | EnteredMarkerGoal::Provisional(result) => {
                return result;
            }
            EnteredMarkerGoal::Active(active) => active,
        };

        // Keep the cyclic root on the stack while revising its provisional
        // result. It is complete only when one evaluation reproduces that
        // value.
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

    /// Returns the rule that decides `goal`, with its premises not yet
    /// evaluated.
    ///
    /// Evaluation and the witness search both select the rule here, so they
    /// cannot disagree about which rule applies.
    async fn marker_rule(&mut self, goal: &MarkerPredicate) -> MarkerRule {
        // The compiler decides these goals itself, before any given is
        // consulted, so a given cannot override them.
        if let Some(rule) = self.built_in_copy_rule(goal).await {
            return rule;
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
                PredicateKind::AssociatedTypeEquality(_)
                | PredicateKind::Marker(_)
                | PredicateKind::Outlives(_) => None,
            })
            // we have to collect here because the `eq_without_unify` call below mutably borrows
            // `self`, which prevents us from using the iterator directly
            .collect::<Vec<_>>();

        for given in matching_givens {
            if self.eq_without_unify(&given, goal.implementor()).await {
                return MarkerRule::Holds;
            }
        }

        // An explicit implementation replaces the synthesized structural rule
        // for its constructor and contributes its substituted marker premises.
        if let Some(rule) = self.explicit_marker_rule(goal).await {
            return rule;
        }

        // Without an explicit rule, synthesize the auto-trait rule from the
        // values stored by the type. Empty structural types prove immediately.
        self.structural_fields(goal).await
    }

    /// Returns the rule of a `Copy` goal whose answer is fixed by the
    /// compiler: linear structs and enums, coroutine types and references.
    ///
    /// Returns `None` when the goal is not one of them, which leaves it to
    /// the givens, the explicit marker rules and structural inference.
    async fn built_in_copy_rule(&self, goal: &MarkerPredicate) -> Option<MarkerRule> {
        if goal.marker_id() != self.engine().get_core_item(CoreItem::Copy).await {
            return None;
        }

        // A linear value must be consumed exactly once, so copying it is never
        // allowed. Types containing it are rejected structurally.
        if let Some(adt_id) = goal.implementor().as_adt_id()
            && self.engine().is_linear_type(adt_id).await
        {
            return Some(MarkerRule::BuiltIn(BuiltInMarkerRule::LinearTypeNotCopy));
        }

        // A coroutine type is never `Copy`, even when a given says so: a copy
        // of a suspended coroutine could be resumed twice, and would still
        // point into the original where a saved local borrows another.
        if goal.implementor().as_coroutine_view().is_some() {
            return Some(MarkerRule::BuiltIn(BuiltInMarkerRule::CoroutineNotCopy));
        }

        // `Copy` for references is built in: a shared reference is always
        // `Copy`, and a unique one never is.
        goal.implementor().as_reference_view().map(|reference| match reference.mutability() {
            Mutability::Immutable => MarkerRule::Holds,
            Mutability::Mutable => MarkerRule::BuiltIn(BuiltInMarkerRule::UniqueReferenceNotCopy),
        })
    }

    async fn prove_active_marker_goal(&mut self, goal: &MarkerPredicate) -> MarkerEntailment {
        // The goal takes the weakest answer among the premises of its rule.
        // Every premise is evaluated, even after one fails, as before the
        // rule was selected separately.
        match self.marker_rule(goal).await {
            MarkerRule::Holds => MarkerEntailment::Holds,
            MarkerRule::CoroutineDeferred => MarkerEntailment::Deferred,
            MarkerRule::BuiltIn(_) | MarkerRule::Negative(_) | MarkerRule::Opaque => {
                MarkerEntailment::Fails
            }
            MarkerRule::Explicit { premises, .. } => {
                let mut entailed = MarkerEntailment::Holds;
                for premise in premises {
                    entailed = entailed.min(self.evaluate_marker_premise(premise).await);
                }
                entailed
            }
            MarkerRule::Structural(premises) => {
                let mut entailed = MarkerEntailment::Holds;
                for (_, premise) in premises {
                    entailed = entailed.min(self.evaluate_marker_goal(premise).await);
                }
                entailed
            }
        }
    }

    /// Evaluates one premise of an explicit marker implementation.
    async fn evaluate_marker_premise(&mut self, premise: PredicateKind) -> MarkerEntailment {
        match premise {
            PredicateKind::Marker(predicate) => self.evaluate_marker_goal(predicate).await,

            // TODO: we'll have to properly investigate this.
            // Sometimes, these predicates are specified because
            // it has to conform to a where-clause requirement
            // when naming the marker head.

            // Lifetimes never decide marker entailment, so
            // an outlives premise, such as a bound implied by
            // the implementor, is not checked here.
            PredicateKind::Outlives(_) => MarkerEntailment::Holds,

            // shouldn't happen because marker implementation predicates can only
            // be marker predicates
            PredicateKind::AssociatedTypeEquality(_) => MarkerEntailment::Fails,
        }
    }

    async fn explicit_marker_rule(&mut self, goal: &MarkerPredicate) -> Option<MarkerRule> {
        let engine = self.engine().clone();
        let target_id = self.site().target_id;
        let implementations =
            engine.get_all_marker_implementations(goal.marker_id(), target_id).await;

        for symbol_id in implementations.iter().copied() {
            let implementation = engine.get_marker_implementation(symbol_id).await;

            let Some(subst) = self
                .simple_head_match(implementation.implementor().clone(), goal.implementor().clone())
                .await
            else {
                continue;
            };

            if implementation.is_negative() {
                return Some(MarkerRule::Negative(symbol_id));
            }

            let clause = engine.get_where_clause(symbol_id).await;

            return Some(MarkerRule::Explicit {
                implementation_id: symbol_id,
                premises: clause
                    .predicates()
                    .map(|predicate| predicate.kind().apply_subst_or_clone(&subst, self.engine()))
                    .collect(),
            });
        }

        None
    }

    fn create_structural_from_components(
        marker_id: GlobalSymbolID,
        components: impl IntoIterator<Item = (StructuralComponent, Interned<Ty>)>,
    ) -> MarkerRule {
        let premises = components
            .into_iter()
            .map(|(component, ty)| (component, MarkerPredicate::new(marker_id, ty)))
            .collect();

        MarkerRule::Structural(premises)
    }

    fn should_defer_coroutine_body_of(&self, def_id: GlobalSymbolID) -> bool {
        self.marker_entailment.visibility == CoroutineSavedLocalsVisibility::Deferred
            && self.site == def_id
    }

    async fn structural_fields_coroutine(
        &self,
        view: &ApplicationView<'_>,
        marker_id: GlobalSymbolID,
    ) -> MarkerRule {
        let (closure_body_id, subst) = match view {
            ApplicationView::ClosureCoroutine(closure) => (
                CoroutineBody::Closure(ClosureCoroutineBodyID::new(
                    closure.closure_view().owner_id(),
                    closure.closure_view().local_closure_id(),
                )),
                closure.closure_view().create_subst(self.engine()).await,
            ),

            ApplicationView::DefCoroutine(def_coro) => {
                (CoroutineBody::Def(def_coro.def_id()), def_coro.create_subst(self.engine()).await)
            }

            _ => unreachable!(),
        };

        if self.should_defer_coroutine_body_of(closure_body_id.def_id()) {
            return MarkerRule::CoroutineDeferred;
        }

        let mut components = Vec::new();

        match closure_body_id {
            CoroutineBody::Def(def_id) => {
                let params = self.engine().get_parameter_map(def_id).await;

                components.extend(params.iter().map(|(_, param)| {
                    (
                        StructuralComponent::CoroutineUpvar(param.span()),
                        param.ty().apply_subst_or_clone(&subst, self.engine()),
                    )
                }));
            }

            CoroutineBody::Closure(closure_coroutine_body_id) => {
                let ir = self.engine().get_ir(closure_body_id.def_id()).await;
                let function_id =
                    ir.get_function_id_of_closure(closure_coroutine_body_id.local_closure_id());
                let function = ir.get_function(function_id);
                let closure_context = function.context().assert_as_closure_context();

                components.extend(
                    closure_context
                        .parameters()
                        .map(|(_, param)| {
                            (
                                StructuralComponent::CoroutineUpvar(Some(param.span())),
                                param.ty().apply_subst_or_clone(&subst, self.engine()),
                            )
                        })
                        .collect::<Vec<_>>(),
                );

                let capture_map = ir
                    .captures_for_function(function_id)
                    .expect("should have a capture map for it");

                components.extend(capture_map.iter().map(|(_, capture)| {
                    (
                        StructuralComponent::CoroutineClosureCapture(capture.span()),
                        capture
                            .storage_ty(self.engine())
                            .apply_subst_or_clone(&subst, self.engine()),
                    )
                }));
            }
        }

        // The values the body holds across its suspension points.
        let saved_locals = self.engine().get_saved_locals(closure_body_id).await;
        for saved_local in saved_locals.as_ref().into_iter().flat_map(|x| x.iter()) {
            let origin = saved_local.origin();
            components.push((
                StructuralComponent::CoroutineSavedLocal(CoroutineSavedLocal::new(
                    origin.kind(),
                    origin.span(),
                    origin.suspension_span(),
                )),
                Ty::erase_lifetimes(
                    &saved_local.ty().apply_subst_or_clone(&subst, self.engine()),
                    self.engine(),
                )
                .await,
            ));
        }

        Self::create_structural_from_components(marker_id, components)
    }

    /// Returns the values whose marker properties determine the enclosing type,
    /// each with where the type stores it. `None` represents an opaque or
    /// ill-kinded type that cannot be inferred.
    ///
    /// A closure stores the tuple of its captures as one value, so that an
    /// explicit marker implementation of tuples applies to it too.
    async fn structural_fields(&self, goal: &MarkerPredicate) -> MarkerRule {
        match &**goal.implementor() {
            Ty::Application(application) => {
                let view = application.view();
                match &view {
                    ApplicationView::Primitive(_) => {
                        Self::create_structural_from_components(goal.marker_id(), [])
                    }

                    ApplicationView::Tuple(tuple) => Self::create_structural_from_components(
                        goal.marker_id(),
                        tuple.args().iter().enumerate().map(|(index, ty)| {
                            (StructuralComponent::TupleElement(index), ty.clone())
                        }),
                    ),

                    ApplicationView::Closure(closure) => Self::create_structural_from_components(
                        goal.marker_id(),
                        std::iter::once((
                            StructuralComponent::ClosureCaptures,
                            closure.captured_tuple().clone(),
                        )),
                    ),

                    ApplicationView::Struct(struct_) => {
                        let body = self.engine().get_struct_body(struct_.symbol_id()).await;
                        let subst = self
                            .engine()
                            .build_subst_from_args(struct_.symbol_id(), struct_.args())
                            .await;

                        Self::create_structural_from_components(
                            goal.marker_id(),
                            body.iter().map(|(field_id, field)| {
                                (
                                    StructuralComponent::StructField(field_id),
                                    field.ty().apply_subst_or_clone(&subst, self.engine()),
                                )
                            }),
                        )
                    }

                    // An enum holds the payload of whichever variant it is, so each
                    // payload type is a field.
                    ApplicationView::Enum(enum_) => {
                        let subst = enum_.create_subst(self.engine()).await;
                        let mut fields = Vec::new();

                        for variant_id in
                            self.engine().get_members(enum_.symbol_id()).await.all_ids()
                        {
                            let variant_id = enum_.symbol_id().target_id.make_global(variant_id);
                            if let Some(payload) =
                                self.engine().get_variant_payload_ty(variant_id).await
                            {
                                fields.push((
                                    StructuralComponent::VariantPayload(variant_id),
                                    payload.apply_subst_or_clone(&subst, self.engine()),
                                ));
                            }
                        }

                        Self::create_structural_from_components(goal.marker_id(), fields)
                    }

                    ApplicationView::ClosureCoroutine(_) | ApplicationView::DefCoroutine(_) => {
                        self.structural_fields_coroutine(&view, goal.marker_id()).await
                    }

                    ApplicationView::Pointer(_)
                    | ApplicationView::Reference(_)
                    | ApplicationView::NoOpDropInstance(_)
                    | ApplicationView::TupleDropInstance(_)
                    | ApplicationView::ClosureDropInstance(_)
                    | ApplicationView::TraitDefCoroutine(_)
                    | ApplicationView::ContinuationDefInstance(_)
                    | ApplicationView::CoroutineInstance(_)
                    | ApplicationView::CoroutineDropInstance(_)
                    | ApplicationView::GeneratedAdtDropInstance(_)
                    | ApplicationView::Instance(_)
                    | ApplicationView::InstanceAssociated(_)
                    | ApplicationView::DefInstance(_)
                    | ApplicationView::Error => MarkerRule::Opaque,
                }
            }
            Ty::Inference(_)
            | Ty::PolyVar(_)
            | Ty::SelfInstance(_)
            | Ty::EffectRow(_)
            | Ty::Lifetime(_) => MarkerRule::Opaque,
        }
    }
}
