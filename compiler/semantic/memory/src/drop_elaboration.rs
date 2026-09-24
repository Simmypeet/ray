use std::cmp::Reverse;

use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Projection},
    cfg::Instruction,
    ir_expr::{IRExpr, IRExprKind, call::Call, load::Load},
    ir_function::{FunctionID, IRFunctionMap},
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::core_item::{CoreItem, get_core_item};
use rayc_type::{capture::LoadKind, subst::Subst, ty::Ty};

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackRoot, StackState, StackStateProblem,
    drop_resolution::{DropFailure, resolve_drop_instance},
};

/// A drop of a stack place selected during a replay, applied to the IR once
/// that replay has finished.
#[derive(Debug)]
pub(crate) struct PendingDrop {
    address: Address,
    ty: Interned<Ty>,

    /// The `Drop` dictionary selected for `ty`.
    drop_instance: Interned<Ty>,

    /// The declaration of the dropped binding, which the inserted
    /// expressions are attributed to.
    span: RelativeSpan,
}

/// Selects the drops that keep stack values from outliving their owners, and
/// the `Drop` dictionaries they use.
///
/// Drops are returned as [`PendingDrop`]s against the analyzed layout and
/// turned into instructions by [`materialize_drops`] once a replay has
/// finished, so inserting them never invalidates the points of the replay.
#[derive(Debug, Default)]
pub(crate) struct DropElaborator {
    /// The selected `Drop` dictionary of each type, or why none is usable.
    instances: FxHashMap<Interned<Ty>, Result<Interned<Ty>, Vec<DropFailure>>>,

    /// Bindings already reported for a type, so a binding which is dropped
    /// on several paths is reported once.
    reported: FxHashSet<(StackRoot, Interned<Ty>)>,
}

impl DropElaborator {
    /// Returns the drops which run immediately before `ScopePop(scope_id)`,
    /// given the stack `state` just before that instruction.
    ///
    /// Once merges are balanced, every place is either initialized or
    /// uninitialized on all paths reaching the scope end. Initialized values
    /// are dropped, and a partially moved aggregate drops each remaining
    /// initialized component instead.
    pub(crate) async fn scope_drops(
        &mut self,
        scope_id: ScopeID,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<PendingDrop> {
        let StackState::Reachable(slots) = state else {
            return Vec::new();
        };

        let mut drops = Vec::new();
        for root in problem.scope_roots_in_drop_order(scope_id).await {
            let place_state = slots.state(root).expect("a live scope root must have a state");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.drop_place(root, address, place_state, ty, problem, &mut drops, diagnostics).await;
        }
        drops
    }

    /// Returns the drops which run just before a value is stored to
    /// `address`, given the stack `state` just before the store.
    ///
    /// Whatever part of the place is still initialized holds a previous value
    /// which would otherwise be overwritten, so it is dropped. Stores through
    /// a dereference are not tracked by the stack state and drop nothing.
    pub(crate) async fn reassignment_drops(
        &mut self,
        address: &Address,
        ty: Interned<Ty>,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<PendingDrop> {
        let (Some(root), Some(place_state)) =
            (StackRoot::from_address_root(address.root()), state.place_state(address))
        else {
            return Vec::new();
        };

        let mut drops = Vec::new();
        self.drop_place(root, address.clone(), place_state, ty, problem, &mut drops, diagnostics)
            .await;
        drops
    }

    /// Returns the drops which balance the stack on a control-flow edge.
    ///
    /// `exit` is the state leaving the edge's source and `entry` is the
    /// joined state at its target. A value which is initialized on this edge
    /// but may be uninitialized on another edge into the same target is
    /// dropped on this edge, so every path agrees on the state at the merge.
    pub(crate) async fn merge_drops(
        &mut self,
        exit: &StackState,
        entry: &StackState,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<PendingDrop> {
        let (StackState::Reachable(exit), StackState::Reachable(entry)) = (exit, entry) else {
            return Vec::new();
        };

        // Order the roots deterministically, dropping later declarations
        // first as a scope exit does.
        let mut roots = entry.roots().collect::<Vec<_>>();

        // REVIEW: okay, this ranking is based on the assupmtion that the `ID`
        // of a `StackRoot` is assigned in the order of declaration, which is
        // quite fragile for me. Perhaps, we should have some way to concretely
        // determines the order of declaration of a `StackRoot` instead of
        // relying on the `ID`. I'm currently thinking of modifying the
        // `rayc_ir`'s Variable to include the notion of declaration order so
        // that we can reliably sort here
        roots.sort_by_key(|root| (drop_order_rank(*root), Reverse(*root)));

        let mut drops = Vec::new();
        for root in roots {
            let entry_state = entry.state(root).expect("a live root must have a state");
            let exit_state = exit.state(root).expect("merging stack states share their live roots");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.balance_place(
                root,
                address,
                entry_state,
                exit_state,
                ty,
                problem,
                &mut drops,
                diagnostics,
            )
            .await;
        }
        drops
    }

    /// Drops the parts of a place which are initialized on the incoming edge
    /// (`exit`) but not on every edge into the merge (`entry`).
    #[allow(clippy::too_many_arguments)]
    async fn balance_place(
        &mut self,
        root: StackRoot,
        address: Address,
        entry: &PlaceState,
        exit: &PlaceState,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        drops: &mut Vec<PendingDrop>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match entry {
            // Every incoming edge, this one included, holds the value.
            PlaceState::Uniform(PossibleStates::Initialized) => {}
            PlaceState::Partial(_) if entry.is_initialized() => {}

            // Some incoming edge lacks the value, so this edge drops
            // whatever part of it this edge still holds.
            PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {
                self.drop_place(root, address, exit, ty, problem, drops, diagnostics).await;
            }

            // Balance each component in reverse order, matching the order of
            // tuple and structural `Drop`.
            PlaceState::Partial(components) => {
                for (projection, entry_component) in components.iter().rev() {
                    // REVIEW: this is quite wasteful isn't it?, `projection_layer` creates a
                    // BTreeMap for every sibling components in the proejction and in this context
                    // we only need the type. Perhaps, we should create a more specialized function
                    // for this use case.
                    let (_, component_ty) = problem.projection_layer(&ty, *projection, entry).await;

                    // REVIEW: can we avoid cloning here?
                    let exit_component = exit.component(*projection);

                    Box::pin(self.balance_place(
                        root,
                        project(&address, *projection, problem.engine()),
                        entry_component,
                        &exit_component,
                        component_ty,
                        problem,
                        drops,
                        diagnostics,
                    ))
                    .await;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn drop_place(
        &mut self,
        root: StackRoot,
        address: Address,
        state: &PlaceState,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        drops: &mut Vec<PendingDrop>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match state {
            // A fully initialized place drops as a whole, so its type's own
            // `Drop` instance decides how the components are dropped.
            PlaceState::Uniform(PossibleStates::Initialized) => {
                self.push_drop(root, address, ty, problem, drops, diagnostics).await;
            }
            PlaceState::Partial(_) if state.is_initialized() => {
                self.push_drop(root, address, ty, problem, drops, diagnostics).await;
            }

            // The value has been moved or was never assigned.
            PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {}

            // Drop the remaining components in reverse order, matching the
            // order of tuple and structural `Drop`.
            PlaceState::Partial(components) => {
                for (projection, component) in components.iter().rev() {
                    // REVIEW: this is quite wasteful isn't it?, `projection_layer` creates a
                    // BTreeMap for every sibling components in the proejction and in this context
                    // we only need the type. Perhaps, we should create a more specialized function
                    // for this use case.
                    let (_, component_ty) = problem.projection_layer(&ty, *projection, state).await;

                    Box::pin(self.drop_place(
                        root,
                        project(&address, *projection, problem.engine()),
                        component,
                        component_ty,
                        problem,
                        drops,
                        diagnostics,
                    ))
                    .await;
                }
            }
        }
    }

    async fn push_drop(
        &mut self,
        root: StackRoot,
        address: Address,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        drops: &mut Vec<PendingDrop>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match self.drop_instance(ty.clone(), problem).await {
            Ok(drop_instance) => {
                let span = problem.binding_span(root).await;
                drops.push(PendingDrop { address, ty, drop_instance, span });
            }

            // Each binding is a separate fix site, but a binding which leaves
            // scope on several paths is reported once.
            Err(failures) => {
                if self.reported.insert((root, ty)) {
                    let span = problem.binding_span(root).await;
                    diagnostics
                        .extend(failures.into_iter().map(|failure| failure.into_diagnostic(span)));
                }
            }
        }
    }

    /// Returns the `Drop` dictionary for `ty`, resolving it on first use.
    async fn drop_instance(
        &mut self,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
    ) -> Result<Interned<Ty>, Vec<DropFailure>> {
        if let Some(resolution) = self.instances.get(&ty) {
            return resolution.clone();
        }

        let resolution = resolve_drop_instance(problem.solver_mut(), ty.clone()).await;
        self.instances.insert(ty, resolution.clone());
        resolution
    }
}

/// Turns drops into instructions of `function_id`: a forced move out of each
/// place followed by a `Drop.drop` call on the moved value.
///
/// The move is forced so a `Copy` value is consumed by its drop as well.
pub(crate) async fn materialize_drops(
    engine: &TrackedEngine,
    functions: &mut IRFunctionMap,
    function_id: FunctionID,
    drops: Vec<PendingDrop>,
) -> Vec<Instruction> {
    let drop_method = engine.get_core_item(CoreItem::DropMethod).await;
    let unit = Ty::new_unit(engine);

    // `Drop.drop` has no effects of its own, so the function's row is a
    // conservative ambient row for the call.

    // TODO: This is an interesting point, this is correct under the assumption
    // that every expression in a function has the same effect as the function
    // itself, which is generally true in the current design of effect system.
    // One might think that the statement "every expression in a function has
    // the same effect as the function itself" is not true since we have a
    // handler expression that can subtract the effect out. However, the
    // handler body is always **a separate function** with its own effect,
    // so the statement is still true.
    //
    // However, if we'll have a borrow-checker feature with lifetimes stuff in the
    // future, then we must first generalize the lifetimes in the effect row of
    // the function before we can use it as the ambient row for the drop call.
    let effect = functions.get_function(function_id).effect().clone();

    let mut instructions = Vec::with_capacity(drops.len() * 2);
    for drop in drops {
        let value = functions.insert_expression(
            function_id,
            IRExpr::new(
                IRExprKind::Load(Load::with_kind(drop.address, LoadKind::Move)),
                drop.span,
                drop.ty,
            ),
        );

        // `Drop.drop` declares no method-local type variables, so its trait
        // call substitution is empty. The unit result is unused and trivially
        // dropped.
        let call = functions.insert_expression(
            function_id,
            IRExpr::new(
                IRExprKind::Call(Call::new_unresolved_instance_associated(
                    drop.drop_instance,
                    drop_method,
                    Subst::new_empty(), /* the `Drop.drop` method has no additional type
                                         * parameters, so its substitution is empty */
                    vec![value],
                    effect.clone(),
                )),
                drop.span,
                unit.clone(),
            ),
        );

        instructions.push(Instruction::Expression(value));
        instructions.push(Instruction::Expression(call));
    }
    instructions
}

/// Extends `address` by one projection.
// REVIEW: Could we abstract this into a method in the `Address` type? since it
// seems like a common operation.
fn project(address: &Address, projection: Projection, engine: &TrackedEngine) -> Address {
    let mut address = address.clone();
    match projection {
        Projection::Tuple(index) => address.add_tuple_index(index, engine),
        Projection::Field(field_id) => address.add_field(field_id, engine),
    }
    address
}

/// Ranks roots so local variables drop before function inputs, and
/// parameters before captures, as they do when the root scope ends.
const fn drop_order_rank(root: StackRoot) -> u8 {
    match root {
        StackRoot::Variable(_) => 0,
        StackRoot::Parameter(_)
        | StackRoot::LambdaParameter(_)
        | StackRoot::OperationHandlerParameter(_) => 1,
        StackRoot::Capture(_) => 2,
    }
}
