//! State tracked by stack-memory dataflow analyses.

use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_semantic_element::{parameter::ParameterID, struct_body::get_struct_body};
use rayc_solver::Solver;
use rayc_type::{
    subst::Substitutable,
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    address::{Address, AddressRoot, Projection},
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, Direction, JoinLattice},
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};

/// CFG locations which may be the most recent move of a place.
///
/// A single execution path has at most one most-recent move, but multiple
/// incoming paths can contribute different locations. The history separately
/// records whether an incoming path was uninitialized without a preceding move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveHistory {
    may_be_uninitialized_without_move: bool,
    points: BTreeSet<Point>,
}

impl MoveHistory {
    /// Creates a history for a place which has not been initialized.
    #[must_use]
    pub const fn uninitialized() -> Self {
        Self { may_be_uninitialized_without_move: true, points: BTreeSet::new() }
    }

    /// Creates a history whose most recent move occurred at `point`.
    #[must_use]
    pub fn moved_at(point: Point) -> Self {
        Self { may_be_uninitialized_without_move: false, points: BTreeSet::from([point]) }
    }

    /// Returns whether no incoming path has a known move location.
    #[must_use]
    pub fn is_empty(&self) -> bool { self.points.is_empty() }

    /// Returns whether an incoming path was uninitialized without being moved.
    #[must_use]
    pub const fn may_be_uninitialized_without_move(&self) -> bool {
        self.may_be_uninitialized_without_move
    }

    /// Iterates over the possible most-recent move locations.
    #[must_use]
    pub fn points(&self) -> impl ExactSizeIterator<Item = Point> + '_ {
        self.points.iter().copied()
    }

    fn join_in_place(&mut self, other: &Self) -> bool {
        let previous_len = self.points.len();
        self.points.extend(other.points.iter().copied());

        let was_uninitialized_without_move = self.may_be_uninitialized_without_move;
        self.may_be_uninitialized_without_move |= other.may_be_uninitialized_without_move;

        previous_len != self.points.len()
            || was_uninitialized_without_move != self.may_be_uninitialized_without_move
    }
}

impl Default for MoveHistory {
    fn default() -> Self { Self::uninitialized() }
}

/// Whether a place is initialized on every path reaching a program point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PossibleStates {
    /// The place is initialized on every incoming path.
    Initialized,

    /// The place is uninitialized on at least one incoming path.
    Uninitialized(MoveHistory),
}

impl PossibleStates {
    /// Creates the state of a place which has not been initialized.
    #[must_use]
    pub const fn uninitialized() -> Self { Self::Uninitialized(MoveHistory::uninitialized()) }

    /// Creates the state produced by moving from a place at `point`.
    #[must_use]
    pub fn moved_at(point: Point) -> Self { Self::Uninitialized(MoveHistory::moved_at(point)) }

    /// Returns whether the place is initialized on every incoming path.
    #[must_use]
    pub const fn is_initialized(&self) -> bool {
        match self {
            Self::Initialized => true,
            Self::Uninitialized(_) => false,
        }
    }

    /// Returns the move history when the place may be uninitialized.
    #[must_use]
    pub const fn move_history(&self) -> Option<&MoveHistory> {
        match self {
            Self::Initialized => None,
            Self::Uninitialized(history) => Some(history),
        }
    }

    #[allow(clippy::match_same_arms)]
    fn join_in_place(&mut self, other: &Self) -> bool {
        match (&mut *self, other) {
            // already the same, so no change
            (Self::Initialized, Self::Initialized) => false,

            // prefer uninitialized over initialized, so no need to update the
            // current state
            (Self::Uninitialized(_), Self::Initialized) => false,

            // becomes uninitialized, so update the current state
            (current @ Self::Initialized, Self::Uninitialized(history)) => {
                *current = Self::Uninitialized(history.clone());
                true
            }

            // merge the histories of two uninitialized states
            (Self::Uninitialized(current), Self::Uninitialized(incoming)) => {
                current.join_in_place(incoming)
            }
        }
    }
}

/// Initialization state of a place, including independently tracked fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaceState {
    /// The place and all of its descendants have the same state.
    Uniform(PossibleStates),

    /// Immediate tuple or struct components have independently tracked states.
    ///
    /// The map contains every immediate component of the aggregate. Keeping it
    /// complete lets the analysis determine when individually initialized
    /// components make the whole aggregate initialized again.
    Partial(BTreeMap<Projection, Self>),
}

#[derive(Clone, Copy)]
enum PlaceUpdate {
    Move,
    Restore,
}

impl PlaceUpdate {
    fn can_traverse(self, state: &PlaceState) -> bool {
        match self {
            // A uniformly uninitialized place has no movable descendant, but
            // a partial place may still contain an initialized component.
            Self::Move => !matches!(state, PlaceState::Uniform(PossibleStates::Uninitialized(_))),

            // An initialized place implies that all descendants are already
            // initialized, so restoring beneath it cannot change the state.
            Self::Restore => !state.is_initialized(),
        }
    }
}

impl PlaceState {
    /// Creates a uniformly initialized place.
    #[must_use]
    pub const fn initialized() -> Self { Self::Uniform(PossibleStates::Initialized) }

    /// Creates a uniformly uninitialized place with no recorded move.
    #[must_use]
    pub const fn uninitialized() -> Self { Self::Uniform(PossibleStates::uninitialized()) }

    /// Creates a uniformly moved place.
    #[must_use]
    pub fn moved_at(point: Point) -> Self { Self::Uniform(PossibleStates::moved_at(point)) }

    /// Creates a place whose immediate components are tracked independently.
    #[must_use]
    pub const fn partial(components: BTreeMap<Projection, Self>) -> Self {
        Self::Partial(components)
    }

    /// Returns whether the entire place is initialized on every incoming path.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        match self {
            Self::Uniform(state) => state.is_initialized(),
            Self::Partial(components) => components.values().all(Self::is_initialized),
        }
    }

    async fn move_at(
        &mut self,
        projections: &[Projection],
        ty: Interned<Ty>,
        point: Point,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Some(selected) =
            self.projected_mut(projections, ty, PlaceUpdate::Move, dataflow_problem_ctx).await
        else {
            return false;
        };

        if !selected.is_initialized() {
            return false;
        }
        *selected = Self::moved_at(point);
        true
    }

    async fn restore_at(
        &mut self,
        projections: &[Projection],
        ty: Interned<Ty>,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Some(selected) =
            self.projected_mut(projections, ty, PlaceUpdate::Restore, dataflow_problem_ctx).await
        else {
            return false;
        };

        if selected.is_initialized() {
            return false;
        }
        *selected = Self::initialized();

        // Canonicalize aggregates whose final uninitialized descendant was
        // restored so future operations can use their uniform state directly.
        self.collapse_initialized();
        true
    }

    async fn projected_mut<'a>(
        &'a mut self,
        projections: &[Projection],
        mut ty: Interned<Ty>,
        update: PlaceUpdate,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> Option<&'a mut Self> {
        let mut selected = self;

        for projection in projections {
            if !update.can_traverse(selected) {
                return None;
            }

            // Resolve and expand only the layer being traversed. This keeps
            // recursive types finite and preserves every sibling's state.
            let (components, projected_ty) =
                dataflow_problem_ctx.projection_layer(&ty, *projection, selected).await;
            if let Some(components) = components {
                *selected = Self::Partial(components);
            }

            let Self::Partial(components) = selected else {
                unreachable!("the uniform aggregate should have been expanded");
            };
            selected = components
                .get_mut(projection)
                .expect("the type-checked projection must exist in the place state");
            ty = projected_ty;
        }

        Some(selected)
    }

    fn collapse_initialized(&mut self) {
        let Self::Partial(components) = self else {
            return;
        };

        for component in components.values_mut() {
            component.collapse_initialized();
        }
        if components
            .values()
            .all(|component| matches!(component, Self::Uniform(PossibleStates::Initialized)))
        {
            *self = Self::initialized();
        }
    }

    fn join_in_place(&mut self, other: &Self) -> bool {
        match (&mut *self, other) {
            (Self::Uniform(current), Self::Uniform(incoming)) => current.join_in_place(incoming),
            (Self::Uniform(uniform), Self::Partial(incoming)) => {
                // Expand the uniform state to the incoming component shape,
                // then join each corresponding component in place.
                let mut components = BTreeMap::new();
                for (projection, incoming) in incoming {
                    let mut state = Self::Uniform(uniform.clone());
                    let _ = state.join_in_place(incoming);
                    components.insert(*projection, state);
                }
                *self = Self::Partial(components);
                true
            }
            (Self::Partial(current), incoming @ Self::Uniform(_)) => current
                .values_mut()
                .fold(false, |changed, state| state.join_in_place(incoming) || changed),

            (Self::Partial(current), Self::Partial(incoming)) => {
                assert_eq!(
                    current.len(),
                    incoming.len(),
                    "partial place states have different component counts",
                );

                current.iter_mut().fold(false, |changed, (projection, current)| {
                    let incoming = incoming
                        .get(projection)
                        .expect("partial place states have different component projections");
                    current.join_in_place(incoming) || changed
                })
            }
        }
    }
}

/// A stack allocation tracked by the memory checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StackRoot {
    Variable(IRVariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    OperationHandlerParameter(OperationHandlerParameterID),
    Capture(CaptureID),
}

impl StackRoot {
    /// Converts an IR address root into a tracked stack allocation.
    ///
    /// Error and dereference roots do not identify storage owned directly by
    /// the current stack frame and are therefore not tracked by this model.
    #[must_use]
    pub const fn from_address_root(root: AddressRoot) -> Option<Self> {
        match root {
            AddressRoot::Variable(variable) => Some(Self::Variable(variable)),
            AddressRoot::Parameter(parameter) => Some(Self::Parameter(parameter)),
            AddressRoot::LambdaParameter(parameter) => Some(Self::LambdaParameter(parameter)),
            AddressRoot::OperationHandlerParameter(parameter) => {
                Some(Self::OperationHandlerParameter(parameter))
            }
            AddressRoot::Capture(capture) => Some(Self::Capture(capture)),
            AddressRoot::Error | AddressRoot::Deref(_) => None,
        }
    }
}

/// Initialization state of the stack allocations in one function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackSlots {
    states: FxHashMap<StackRoot, PlaceState>,
}

impl StackSlots {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Inserts or replaces the state of a stack allocation.
    pub fn set(&mut self, root: StackRoot, state: PlaceState) {
        let _ = self.states.insert(root, state);
    }

    /// Returns the state of a stack allocation.
    #[must_use]
    pub fn state(&self, root: StackRoot) -> Option<&PlaceState> { self.states.get(&root) }

    fn state_mut(&mut self, root: StackRoot) -> Option<&mut PlaceState> {
        self.states.get_mut(&root)
    }
}

/// Memory-checker facts at a point in the control-flow graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackState {
    /// Lattice bottom: no control-flow path reaches this point.
    Unreachable,

    /// States propagated by paths which reach this point.
    Reachable(StackSlots),
}

impl StackState {
    /// Creates reachable state with no registered stack allocations.
    #[must_use]
    pub fn reachable() -> Self { Self::Reachable(StackSlots::new()) }

    /// Returns whether at least one control-flow path reaches this state.
    #[must_use]
    pub const fn is_reachable(&self) -> bool {
        match self {
            Self::Unreachable => false,
            Self::Reachable(_) => true,
        }
    }

    /// Moves from `address` and records `point` as its most recent move.
    ///
    /// Returns `false` without changing the state when the address is not a
    /// tracked stack place or the selected place is not fully initialized.
    pub async fn move_out(
        &mut self,
        address: &Address,
        point: Point,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Some(root) = StackRoot::from_address_root(address.root()) else {
            return false;
        };

        self.move_place(root, address.projections(), point, dataflow_problem_ctx).await
    }

    /// Moves from a stack root and projection path.
    ///
    /// This lower-level form is useful when an analysis already decomposed an
    /// [`Address`] into its stack root and projections.
    pub async fn move_place(
        &mut self,
        root: StackRoot,
        projections: &[Projection],
        point: Point,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Self::Reachable(slots) = self else {
            return false;
        };
        let ty = dataflow_problem_ctx
            .binding_type(root)
            .expect("tracked stack root must have a registered type");
        let state = slots.state_mut(root).expect("tracked stack root must have a state");

        state.move_at(projections, ty, point, dataflow_problem_ctx).await
    }

    /// Restores `address` to the initialized state after assigning to it.
    ///
    /// Returns whether the tracked state changed. Assignments through an
    /// untracked address do not change the stack state.
    pub async fn restore(
        &mut self,
        address: &Address,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Some(root) = StackRoot::from_address_root(address.root()) else {
            return false;
        };

        self.restore_place(root, address.projections(), dataflow_problem_ctx).await
    }

    /// Restores a stack root and projection path after assigning to it.
    ///
    /// This lower-level form is useful when an analysis already decomposed an
    /// [`Address`] into its stack root and projections.
    pub async fn restore_place(
        &mut self,
        root: StackRoot,
        projections: &[Projection],
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let Self::Reachable(slots) = self else {
            return false;
        };
        let ty = dataflow_problem_ctx
            .binding_type(root)
            .expect("tracked stack root must have a registered type");
        let state = slots.state_mut(root).expect("tracked stack root must have a state");

        state.restore_at(projections, ty, dataflow_problem_ctx).await
    }
}

/// Dataflow context for stack initialization and move state.
///
/// It stores only each stack root's type. Types are normalized and aggregate
/// structure is queried lazily when an address projects into that aggregate,
/// so recursive types are never expanded eagerly.
#[derive(Debug)]
pub struct StackStateProblem {
    solver: Solver,
    bindings: FxHashMap<StackRoot, Interned<Ty>>,
}

impl StackStateProblem {
    #[must_use]
    pub fn new(solver: Solver) -> Self { Self { solver, bindings: FxHashMap::default() } }

    /// Registers the type of a stack allocation.
    pub fn register(&mut self, root: StackRoot, ty: Interned<Ty>) {
        let _ = self.bindings.insert(root, ty);
    }

    fn binding_type(&self, root: StackRoot) -> Option<Interned<Ty>> {
        self.bindings.get(&root).cloned()
    }

    /// Resolves one projection into its component states and selected type.
    ///
    /// [`PlaceState::move_at`] calls this immediately before traversing each
    /// projection. When `place_state` is uniform, this directly constructs the
    /// component map used to expand it, avoiding an intermediate collection of
    /// sibling projections. An already-partial place returns no replacement
    /// map. The selected type becomes the input to the next projection.
    ///
    /// The type is normalized before its shape is inspected. Struct bodies and
    /// generic field substitutions are queried only for the current
    /// projection. Types beneath fields not selected by the address are never
    /// traversed, and no collection of future layers is built, which keeps
    /// recursive types finite.
    ///
    /// # Panics
    ///
    /// Panics if the normalized type is not an application, the projection
    /// kind does not match the type, or the requested component does not exist.
    async fn projection_layer(
        &self,
        ty: &Interned<Ty>,
        projection: Projection,
        place_state: &PlaceState,
    ) -> (Option<BTreeMap<Projection, PlaceState>>, Interned<Ty>) {
        let ty = self.solver.normalize(ty).await;
        let Ty::Application(application) = &*ty else {
            panic!("projected type must normalize to an application: {ty:?}");
        };

        match (application.view(), projection) {
            (ApplicationView::Tuple(tuple), Projection::Tuple(index)) => {
                let projected = tuple
                    .args()
                    .get(index)
                    .unwrap_or_else(|| panic!("tuple projection index {index} is out of bounds"))
                    .clone();
                let components = match place_state {
                    PlaceState::Uniform(state) => Some(
                        (0..tuple.args().len())
                            .map(|index| {
                                (Projection::Tuple(index), PlaceState::Uniform(state.clone()))
                            })
                            .collect(),
                    ),
                    PlaceState::Partial(_) => None,
                };
                (components, projected)
            }
            (ApplicationView::Struct(struct_ty), Projection::Field(field_id)) => {
                let engine = self.solver.engine();
                let substitution = struct_ty.create_subst(engine).await;
                let body = engine.get_struct_body(struct_ty.symbol_id()).await;
                let field = body
                    .iter()
                    .find_map(|(id, field)| (id == field_id).then_some(field))
                    .unwrap_or_else(|| {
                        panic!("struct projection references missing field {field_id:?}")
                    });
                let projected = field.ty().apply_subst_or_clone(&substitution, engine);
                let components = match place_state {
                    PlaceState::Uniform(state) => Some(
                        body.iter()
                            .map(|(id, _)| {
                                (Projection::Field(id), PlaceState::Uniform(state.clone()))
                            })
                            .collect(),
                    ),
                    PlaceState::Partial(_) => None,
                };
                (components, projected)
            }
            (
                ApplicationView::Primitive(_)
                | ApplicationView::Pointer(_)
                | ApplicationView::Instance(_)
                | ApplicationView::InstanceAssociated(_)
                | ApplicationView::Closure(_)
                | ApplicationView::DefInstance(_)
                | ApplicationView::Error,
                Projection::Tuple(_) | Projection::Field(_),
            )
            | (ApplicationView::Tuple(_), Projection::Field(_))
            | (ApplicationView::Struct(_), Projection::Tuple(_)) => {
                panic!("projection {projection:?} does not match normalized type {ty:?}");
            }
        }
    }
}

impl JoinLattice<StackStateProblem, Infallible> for StackState {
    #[allow(clippy::match_same_arms)]
    async fn join(
        &mut self,
        other: &Self,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> Result<bool, Infallible> {
        match (&mut *self, other) {
            (Self::Unreachable, Self::Unreachable) => Ok(false),
            (Self::Reachable(_), Self::Unreachable) => Ok(false),

            (current @ Self::Unreachable, Self::Reachable(_)) => {
                *current = other.clone();
                Ok(true)
            }
            (Self::Reachable(current), Self::Reachable(incoming)) => {
                assert_eq!(
                    current.states.len(),
                    dataflow_problem_ctx.bindings.len(),
                    "current stack state does not match the type-checked bindings",
                );
                assert_eq!(
                    incoming.states.len(),
                    dataflow_problem_ctx.bindings.len(),
                    "incoming stack state does not match the type-checked bindings",
                );

                // The type-checked IR guarantees matching roots and place
                // shapes, so update each state directly in place.
                let mut changed = false;
                for (root, current) in &mut current.states {
                    assert!(
                        dataflow_problem_ctx.bindings.contains_key(root),
                        "stack state contains a root without a type-checked binding: {root:?}",
                    );
                    let incoming =
                        incoming.state(*root).expect("incoming stack state is missing a root");
                    changed |= current.join_in_place(incoming);
                }
                Ok(changed)
            }
        }
    }
}

impl DataflowProblem for StackStateProblem {
    type JoinLattice = StackState;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<StackState, Infallible> {
        Ok(StackState::Unreachable)
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<StackState, Infallible> {
        let mut slots = StackSlots::new();
        for root in self.bindings.keys() {
            let state = match root {
                StackRoot::Variable(_) => PlaceState::uninitialized(),
                StackRoot::Parameter(_)
                | StackRoot::LambdaParameter(_)
                | StackRoot::OperationHandlerParameter(_)
                | StackRoot::Capture(_) => PlaceState::initialized(),
            };
            slots.set(*root, state);
        }
        Ok(StackState::Reachable(slots))
    }

    async fn transfer_instruction(
        &mut self,
        _point: Point,
        _instruction: &Instruction,
        _state: &mut StackState,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut StackState,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut StackState,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
