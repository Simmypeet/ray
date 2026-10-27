use qbice::storage::intern::Interned;
use rayc_hash::{FxHashMap, FxHashSet};
use rayc_ir::{
    address::{Address, Local},
    cfg::{
        AddressDrop, BlockID, ControlFlowEdge, ExprDiscard, Instruction, InstructionInsertion,
        Point, Terminator,
    },
    ir_expr::IRExprID,
    ir_function::{FunctionID, IRFunction, IRFunctionMap},
    scope::ScopeID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    Diagnostic, PlaceState, PossibleStates, StackState, StackStateProblem,
    analyze_result::AnalyzeResult,
    drop_resolution::{DropInstance, DroppedValue, resolve_drop_instance},
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

/// Selects the drops that keep stack values, and evaluated values that are
/// never consumed, from outliving their owners, and the `Drop` dictionaries
/// they use.
///
/// Every drop of a function is collected against the analyzed layout and
/// inserted at once by [`Self::insert_drops`] after the replay, so inserting
/// them never invalidates the points of the replay.
#[derive(Debug)]
pub(crate) struct DropElaborator<'x> {
    /// The selected drops, in the order they run at each site: an
    /// [`Instruction::AddressDrop`] of a place, attributed to the declaration
    /// of the dropped binding, or an [`Instruction::ExprDiscard`] of a value.
    drops: Vec<(DropSite, Instruction)>,

    /// The selected `Drop` dictionary of each type which has a usable one.
    ///
    /// A type without one is resolved again for each binding or value it is
    /// reported against, so its failure is never stored.
    instances: FxHashMap<Interned<Ty>, DropInstance>,

    /// Bindings already reported for a type, so a binding which is dropped
    /// on several paths is resolved and reported once.
    reported: FxHashSet<(Local, Interned<Ty>)>,

    /// Values already reported, so a value which is dropped on several paths
    /// is resolved and reported once.
    reported_values: FxHashSet<IRExprID>,

    /// The abandon block of each suspension point, with the source of that
    /// suspension point. What such a block drops is held across it.
    abandon_blocks: FxHashMap<BlockID, RelativeSpan>,

    analyze_result: &'x mut AnalyzeResult,
}

impl<'x> DropElaborator<'x> {
    /// Creates an elaborator for the drops of `function`.
    pub(crate) fn new(function: &IRFunction, analyze_result: &'x mut AnalyzeResult) -> Self {
        let abandon_blocks = function
            .reachables()
            .blocks()
            .filter_map(|block_id| {
                let abandon_block = function.block_terminator(block_id)?.abandon_block()?;
                let span = function
                    .point_span(function.terminator_point(block_id))
                    .expect("a suspension point evaluates an expression");
                Some((abandon_block, span))
            })
            .collect();

        Self {
            abandon_blocks,
            drops: Vec::new(),
            instances: FxHashMap::default(),
            reported: FxHashSet::default(),
            reported_values: FxHashSet::default(),
            analyze_result,
        }
    }

    /// Returns the source of the suspension point that a drop at `site` runs
    /// after, when the function is dropped while it is suspended there.
    fn suspension_span(&self, site: DropSite) -> Option<RelativeSpan> {
        match site {
            DropSite::Before(point) => self.abandon_blocks.get(&point.block_id()).copied(),

            // An abandon block has only its suspension point as predecessor,
            // so no edge into one is balanced.
            DropSite::Edge(_) => None,
        }
    }

    pub(crate) fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.analyze_result.extend_diagnostics(std::iter::once(diagnostic));
    }

    /// Selects the drops which run immediately before the
    /// `ScopePop(scope_id)` at `point`, given the stack `state` just before
    /// that instruction.
    ///
    /// Once merges are balanced, every place is either initialized or
    /// uninitialized on all paths reaching the scope end. Initialized values
    /// are dropped, and a partially moved aggregate drops each remaining
    /// initialized component instead.
    ///
    /// Before them, the values evaluated in the scope which are never
    /// consumed are dropped, newest first. Such a value is left behind when
    /// control leaves in the middle of an evaluation, like the earlier
    /// arguments of a call whose later argument returns. The values are the
    /// newest things the scope owns, and may hold borrows of its variables.
    pub(crate) async fn scope_drops(
        &mut self,
        point: Point,
        scope_id: ScopeID,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
    ) {
        let StackState::Reachable(slots) = state else {
            return;
        };

        let site = DropSite::Before(point);

        // Values the scope owns, which are no longer used past its end.
        for value in slots.values_owned_by(scope_id) {
            if !problem.is_value_live(value, point) {
                self.discard_value(site, value, problem).await;
            }
        }

        // Then the variables declared in the scope.
        for root in problem.scope_roots_in_drop_order(scope_id).await {
            let place_state = slots.state(root).expect("a live scope root must have a state");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.drop_place(site, root, address, place_state, ty, problem).await;
        }
    }

    /// Selects the drops which run just before a value is stored to
    /// `address` by the `Store` at `point`, given the stack `state` just
    /// before the store.
    ///
    /// Whatever part of the place is still initialized holds a previous value
    /// which would otherwise be overwritten, so it is dropped.
    ///
    /// A place behind a reference is borrowed memory, which always holds an
    /// initialized value, so its previous value is dropped as a whole. A
    /// place behind a raw pointer is untracked memory and drops nothing.
    pub(crate) async fn reassignment_drops(
        &mut self,
        point: Point,
        address: &Address,
        ty: Interned<Ty>,
        state: &StackState,
        problem: &mut StackStateProblem<'_>,
    ) {
        let site = DropSite::Before(point);

        if address.is_behind_deref() {
            if let Some(root) = address.local()
                && address.is_behind_reference()
            {
                self.push_drop(site, root, address.clone(), ty, problem).await;
            }
            return;
        }

        let (Some(root), Some(place_state)) = (address.direct_local(), state.place_state(address))
        else {
            return;
        };

        self.drop_place(site, root, address.clone(), place_state, ty, problem).await;
    }

    /// Selects the drops which balance the stack on a control-flow edge.
    ///
    /// `exit` is the state leaving the edge's source and `entry` is the
    /// joined state at its target. A value which is initialized on this edge
    /// but may be uninitialized on another edge into the same target is
    /// dropped on this edge, so every path agrees on the state at the merge.
    /// Values are considered in `drop_order`, the order a scope exit drops
    /// them.
    ///
    /// Likewise, an evaluated value which this edge still owns but another
    /// edge into the target does not is dropped on this edge, before any
    /// place.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn merge_drops(
        &mut self,
        edge: ControlFlowEdge,
        exit: &StackState,
        entry: &StackState,
        drop_order: &[Local],
        problem: &mut StackStateProblem<'_>,
    ) {
        let (StackState::Reachable(exit), StackState::Reachable(entry)) = (exit, entry) else {
            return;
        };

        let site = DropSite::Edge(edge);

        // Values owned on this edge but not at the merge, newest first.
        for value in exit.pending_values() {
            if !entry.is_pending(value) {
                self.discard_value(site, value, problem).await;
            }
        }

        // Then the places, in drop order.
        for root in drop_order.iter().copied() {
            // Only roots live at the merge can need balancing.
            let Some(entry_state) = entry.state(root) else {
                continue;
            };
            let exit_state = exit.state(root).expect("merging stack states share their live roots");
            let ty = problem.binding_type(root).await;
            let address = root.to_address(problem.engine());

            self.balance_place(site, root, address, entry_state, exit_state, ty, problem).await;
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
    ) {
        match entry {
            // Every incoming edge, this one included, holds the value.
            PlaceState::Uniform(PossibleStates::Initialized) => {}
            PlaceState::Partial(_) if entry.is_initialized() => {}

            // Some incoming edge lacks the value, so this edge drops
            // whatever part of it this edge still holds.
            PlaceState::Uniform(PossibleStates::Uninitialized(_)) => {
                self.drop_place(site, root, address, exit, ty, problem).await;
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
    ) {
        match state {
            // A fully initialized place drops as a whole, so its type's own
            // `Drop` instance decides how the components are dropped.
            PlaceState::Uniform(PossibleStates::Initialized) => {
                self.push_drop(site, root, address, ty, problem).await;
            }
            PlaceState::Partial(_) if state.is_initialized() => {
                self.push_drop(site, root, address, ty, problem).await;
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
                    ))
                    .await;
                }
            }
        }
    }

    /// Drops the value held in `address` at `site` with an
    /// [`Instruction::AddressDrop`].
    ///
    /// A place whose drop does nothing, such as a `Copy` one, needs no drop.
    /// The stack state is solved before any drop is inserted, so leaving the
    /// drop out does not change which places are initialized.
    async fn push_drop(
        &mut self,
        site: DropSite,
        root: Local,
        address: Address,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
    ) {
        // Each binding is a separate fix site, but a binding which leaves
        // scope on several paths is reported once.
        if self.reported.contains(&(root, ty.clone())) {
            return;
        }

        let span = problem.binding_span(root).await;
        let Some(drop_instance) = self.drop_instance(site, span, ty.clone(), problem).await else {
            self.reported.insert((root, ty));
            return;
        };

        self.analyze_result.extend_deferred_markers(drop_instance.deferred_markers(span));

        let drop_instance = drop_instance.instance();
        if !drop_instance.is_no_op_drop_instance() {
            let drop = AddressDrop::new(address, drop_instance.clone(), span);
            self.drops.push((site, Instruction::AddressDrop(drop)));
        }
    }

    /// Drops the evaluated `value` at `site` with an
    /// [`Instruction::ExprDiscard`].
    ///
    /// A value whose drop does nothing, such as a `Copy` one, needs no
    /// discard.
    async fn discard_value(
        &mut self,
        site: DropSite,
        value: IRExprID,
        problem: &mut StackStateProblem<'_>,
    ) {
        // A value which is left behind on several paths is reported once.
        if self.reported_values.contains(&value) {
            return;
        }

        let span = problem.value_span(value);
        let ty = problem.value_type(value);
        let Some(drop_instance) = self.drop_instance(site, span, ty, problem).await else {
            self.reported_values.insert(value);
            return;
        };

        self.analyze_result.extend_deferred_markers(drop_instance.deferred_markers(span));

        let drop_instance = drop_instance.instance();
        if !drop_instance.is_no_op_drop_instance() {
            let discard = ExprDiscard::new(value, drop_instance.clone());
            self.drops.push((site, Instruction::ExprDiscard(discard)));
        }
    }

    /// Inserts every selected drop into `function_id`.
    ///
    /// A drop on an edge runs where it runs only when control follows that
    /// edge: at the end of the source block when the edge is the source's
    /// only one, and otherwise in a new block which splits the edge.
    pub(crate) fn insert_drops(self, functions: &mut IRFunctionMap, function_id: FunctionID) {
        let mut insertion = InstructionInsertion::new();
        let mut edge_points = FxHashMap::<ControlFlowEdge, Point>::default();
        for (site, instruction) in self.drops {
            let point = match site {
                DropSite::Before(point) => point,
                DropSite::Edge(edge) => *edge_points
                    .entry(edge)
                    .or_insert_with(|| edge_drop_point(functions, function_id, edge)),
            };

            insertion.insert_before(point, [instruction]);
        }

        functions.insert_instructions(function_id, insertion);
    }

    /// Returns the `Drop` dictionary for `ty`, resolving it on first use.
    ///
    /// When `ty` has no usable dictionary, reports why against the value
    /// declared or evaluated at `span`, dropped at `site`, and returns `None`.
    async fn drop_instance(
        &mut self,
        site: DropSite,
        span: RelativeSpan,
        ty: Interned<Ty>,
        problem: &mut StackStateProblem<'_>,
    ) -> Option<DropInstance> {
        if let Some(drop_instance) = self.instances.get(&ty) {
            return Some(drop_instance.clone());
        }

        let dropped = DroppedValue { span, suspension: self.suspension_span(site) };
        let drop_instance = resolve_drop_instance(
            problem.solver_mut(),
            ty.clone(),
            dropped,
            &mut *self.analyze_result,
        )
        .await?;
        self.instances.insert(ty, drop_instance.clone());
        Some(drop_instance)
    }
}

/// Returns the point where drops on `edge` run: the end of the source block
/// when the edge leaves through a jump, or a new block splitting the edge
/// otherwise.
fn edge_drop_point(
    functions: &mut IRFunctionMap,
    function_id: FunctionID,
    edge: ControlFlowEdge,
) -> Point {
    // A jump is the source's only edge, so the drops can end the source
    // block. A conditional edge is critical, since its target merges several
    // edges, so it gets a block of its own. So does an edge out of a
    // suspension point: the end of the source block is before its expression
    // is evaluated, not after. The edges out of a `Handle` are both: it has
    // several successors, and the coroutine runs before any of them.
    let terminator = functions.get_function(function_id).block_terminator(edge.source());
    let block_id = match terminator {
        Some(Terminator::Jump(_)) => edge.source(),
        Some(Terminator::Conditional(_) | Terminator::Suspend(_) | Terminator::Handle(_)) => {
            functions.split_edge(function_id, edge)
        }
        Some(Terminator::Return(_) | Terminator::Abandon) | None => {
            unreachable!("a control-flow edge does not leave through a return")
        }
    };

    let instruction_idx = functions.get_function(function_id).block_instructions(block_id).len();
    Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
}
