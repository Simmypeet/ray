use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Local},
    cfg::{AddressDrop, ControlFlowEdge, Instruction, InstructionInsertion, Point, Terminator},
    ir_function::{FunctionID, IRFunctionMap},
    scope::ScopeID,
};
use rayc_type::ty::Ty;

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackState, StackStateProblem,
    drop_resolution::{DropFailure, resolve_drop_instance},
};

/// Where a selected drop runs.
#[derive(Debug, Clone, Copy)]
enum DropSite {
    /// Immediately before the instruction at this point of the analyzed
    /// layout.
    Before(Point),

    /// Only when control follows this edge.
    Edge(ControlFlowEdge),
}

/// Selects the drops that keep stack values from outliving their owners, and
/// the `Drop` dictionaries they use.
///
/// Every drop of a function is collected against the analyzed layout and
/// inserted at once by [`Self::insert_drops`] after the replay, so inserting
/// them never invalidates the points of the replay.
#[derive(Debug, Default)]
pub(crate) struct DropElaborator {
    /// The selected drops, in the order they run at each site. Each is
    /// attributed to the declaration of the dropped binding.
    drops: Vec<(DropSite, AddressDrop)>,

    /// The selected `Drop` dictionary of each type, or why none is usable.
    instances: FxHashMap<Interned<Ty>, Result<Interned<Ty>, Vec<DropFailure>>>,

    /// Bindings already reported for a type, so a binding which is dropped
    /// on several paths is reported once.
    reported: FxHashSet<(Local, Interned<Ty>)>,
}

impl DropElaborator {
    /// Selects the drops which run immediately before the
    /// `ScopePop(scope_id)` at `point`, given the stack `state` just before
    /// that instruction.
    ///
    /// Once merges are balanced, every place is either initialized or
    /// uninitialized on all paths reaching the scope end. Initialized values
    /// are dropped, and a partially moved aggregate drops each remaining
    /// initialized component instead.
    pub(crate) async fn scope_drops(
        &mut self,
        point: Point,
        scope_id: ScopeID,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let StackState::Reachable(slots) = state else {
            return;
        };

        let site = DropSite::Before(point);
        for root in problem.scope_roots_in_drop_order(scope_id).await {
            let place_state = slots.state(root).expect("a live scope root must have a state");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.drop_place(site, root, address, place_state, ty, problem, diagnostics).await;
        }
    }

    /// Selects the drops which run just before a value is stored to
    /// `address` by the `Store` at `point`, given the stack `state` just
    /// before the store.
    ///
    /// Whatever part of the place is still initialized holds a previous value
    /// which would otherwise be overwritten, so it is dropped. Stores through
    /// a dereference are not tracked by the stack state and drop nothing.
    pub(crate) async fn reassignment_drops(
        &mut self,
        point: Point,
        address: &Address,
        ty: Interned<Ty>,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let (Some(root), Some(place_state)) = (address.direct_local(), state.place_state(address))
        else {
            return;
        };

        let site = DropSite::Before(point);
        self.drop_place(site, root, address.clone(), place_state, ty, problem, diagnostics).await;
    }

    /// Selects the drops which balance the stack on a control-flow edge.
    ///
    /// `exit` is the state leaving the edge's source and `entry` is the
    /// joined state at its target. A value which is initialized on this edge
    /// but may be uninitialized on another edge into the same target is
    /// dropped on this edge, so every path agrees on the state at the merge.
    /// Values are considered in `drop_order`, the order a scope exit drops
    /// them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn merge_drops(
        &mut self,
        edge: ControlFlowEdge,
        exit: &StackState,
        entry: &StackState,
        drop_order: &[Local],
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let (StackState::Reachable(exit), StackState::Reachable(entry)) = (exit, entry) else {
            return;
        };

        let site = DropSite::Edge(edge);
        for root in drop_order.iter().copied() {
            // Only roots live at the merge can need balancing.
            let Some(entry_state) = entry.state(root) else {
                continue;
            };
            let exit_state = exit.state(root).expect("merging stack states share their live roots");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.balance_place(
                site,
                root,
                address,
                entry_state,
                exit_state,
                ty,
                problem,
                diagnostics,
            )
            .await;
        }
    }

    /// Drops the parts of a place which are initialized on the incoming edge
    /// (`exit`) but not on every edge into the merge (`entry`).
    #[allow(clippy::too_many_arguments)]
    async fn balance_place(
        &mut self,
        site: DropSite,
        root: Local,
        address: Address,
        entry: &PlaceState,
        exit: &PlaceState,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match entry {
            // Every incoming edge, this one included, holds the value.
            PlaceState::Uniform(PossibleStates::Initialized) => {}
            PlaceState::Partial(_) if entry.is_initialized() => {}

            // Some incoming edge lacks the value, so this edge drops
            // whatever part of it this edge still holds.
            PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {
                self.drop_place(site, root, address, exit, ty, problem, diagnostics).await;
            }

            // Balance each component in reverse order, matching the order of
            // tuple and structural `Drop`.
            PlaceState::Partial(components) => {
                for (projection, entry_component) in components.iter().rev() {
                    let component_ty = problem.projected_type(&ty, *projection).await;

                    Box::pin(self.balance_place(
                        site,
                        root,
                        address.projected(*projection, problem.engine()),
                        entry_component,
                        exit.component(*projection),
                        component_ty,
                        problem,
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
        site: DropSite,
        root: Local,
        address: Address,
        state: &PlaceState,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match state {
            // A fully initialized place drops as a whole, so its type's own
            // `Drop` instance decides how the components are dropped.
            PlaceState::Uniform(PossibleStates::Initialized) => {
                self.push_drop(site, root, address, ty, problem, diagnostics).await;
            }
            PlaceState::Partial(_) if state.is_initialized() => {
                self.push_drop(site, root, address, ty, problem, diagnostics).await;
            }

            // The value has been moved or was never assigned.
            PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {}

            // Drop the remaining components in reverse order, matching the
            // order of tuple and structural `Drop`.
            PlaceState::Partial(components) => {
                for (projection, component) in components.iter().rev() {
                    let component_ty = problem.projected_type(&ty, *projection).await;

                    Box::pin(self.drop_place(
                        site,
                        root,
                        address.projected(*projection, problem.engine()),
                        component,
                        component_ty,
                        problem,
                        diagnostics,
                    ))
                    .await;
                }
            }
        }
    }

    async fn push_drop(
        &mut self,
        site: DropSite,
        root: Local,
        address: Address,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match self.drop_instance(ty.clone(), problem).await {
            Ok(drop_instance) => {
                let span = problem.binding_span(root).await;
                self.drops.push((site, AddressDrop::new(address, drop_instance, span)));
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

    /// Inserts every selected drop into `function_id` as an
    /// [`Instruction::AddressDrop`].
    ///
    /// A drop on an edge runs where it runs only when control follows that
    /// edge: at the end of the source block when the edge is the source's
    /// only one, and otherwise in a new block which splits the edge.
    pub(crate) fn insert_drops(self, functions: &mut IRFunctionMap, function_id: FunctionID) {
        let mut insertion = InstructionInsertion::new();
        let mut edge_points = FxHashMap::<ControlFlowEdge, Point>::default();
        for (site, drop) in self.drops {
            let point = match site {
                DropSite::Before(point) => point,
                DropSite::Edge(edge) => *edge_points
                    .entry(edge)
                    .or_insert_with(|| edge_drop_point(functions, function_id, edge)),
            };

            insertion.insert_before(point, [Instruction::AddressDrop(drop)]);
        }

        functions.insert_instructions(function_id, insertion);
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

/// Returns the point where drops on `edge` run: the end of the source block
/// when the edge is the source's only one, or a new block splitting the edge
/// otherwise.
fn edge_drop_point(
    functions: &mut IRFunctionMap,
    function_id: FunctionID,
    edge: ControlFlowEdge,
) -> Point {
    // A jump is the source's only edge, so the drops can end the source
    // block. A conditional edge is critical, since its target merges several
    // edges, so it gets a block of its own.
    let terminator = functions.get_function(function_id).block_terminator(edge.source());
    let block_id = match terminator {
        Some(Terminator::Jump(_)) => edge.source(),
        Some(Terminator::Conditional(_)) => functions.split_edge(function_id, edge),
        Some(Terminator::Return(_)) | None => {
            unreachable!("a control-flow edge leaves through a jump or a conditional")
        }
    };

    let instruction_idx = functions.get_function(function_id).block_instructions(block_id).len();
    Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
}
