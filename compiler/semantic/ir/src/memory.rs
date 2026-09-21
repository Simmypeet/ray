//! State tracked by stack-memory dataflow analyses.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::ParameterID, struct_body::get_struct_body};
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

    fn joined(&self, other: &Self) -> Self {
        let mut points = self.points.clone();
        points.extend(other.points.iter().copied());

        Self {
            may_be_uninitialized_without_move: self.may_be_uninitialized_without_move
                || other.may_be_uninitialized_without_move,
            points,
        }
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

    fn joined(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::Initialized, Self::Initialized) => Self::Initialized,
            (Self::Initialized, Self::Uninitialized(history))
            | (Self::Uninitialized(history), Self::Initialized) => {
                Self::Uninitialized(history.clone())
            }
            (Self::Uninitialized(left), Self::Uninitialized(right)) => {
                Self::Uninitialized(left.joined(right))
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
        mut ty: Interned<Ty>,
        point: Point,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> bool {
        let mut selected = self;

        for projection in projections {
            // A uniform uninitialized state applies to every descendant, so
            // no deeper projection can identify a movable place. A partial
            // state must still be traversed because another child may remain
            // initialized.
            match selected {
                Self::Uniform(PossibleStates::Uninitialized(_)) => return false,
                Self::Uniform(PossibleStates::Initialized) | Self::Partial(_) => {}
            }

            // Resolve only the layer that is about to be traversed. This keeps
            // recursive types finite without collecting every layer first.
            let Some((components, projected_ty)) =
                dataflow_problem_ctx.projection_layer(&ty, *projection, selected).await
            else {
                return false;
            };

            // Expand a uniform aggregate only when one of its components
            // changes, preserving the current state for every sibling.
            if let Some(components) = components {
                *selected = Self::Partial(components);
            }

            let Self::Partial(components) = selected else {
                unreachable!("the uniform aggregate should have been expanded");
            };
            let Some(component) = components.get_mut(projection) else {
                return false;
            };

            selected = component;
            ty = projected_ty;
        }

        // A non-empty projection path returns from its final iteration above,
        // so only a move of the root place reaches this point.
        if !selected.is_initialized() {
            return false;
        }
        *selected = Self::moved_at(point);
        true
    }

    fn joined(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::Uniform(left), Self::Uniform(right)) => Some(Self::Uniform(left.joined(right))),
            (Self::Uniform(_), Self::Partial(right)) => {
                let mut joined_components = BTreeMap::new();
                for (projection, right) in right {
                    joined_components.insert(*projection, self.joined(right)?);
                }
                Some(Self::Partial(joined_components))
            }
            (Self::Partial(left), Self::Uniform(_)) => {
                let mut joined_components = BTreeMap::new();
                for (projection, left) in left {
                    joined_components.insert(*projection, left.joined(other)?);
                }
                Some(Self::Partial(joined_components))
            }
            (Self::Partial(left), Self::Partial(right)) => {
                if left.len() != right.len() || !left.keys().eq(right.keys()) {
                    return None;
                }

                let mut joined_components = BTreeMap::new();
                for ((projection, left), right) in left.iter().zip(right.values()) {
                    joined_components.insert(*projection, left.joined(right)?);
                }
                Some(Self::Partial(joined_components))
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
        let Some(ty) = dataflow_problem_ctx.binding_type(root) else {
            return false;
        };
        let Some(state) = slots.state(root) else {
            return false;
        };

        // Traverse and update a clone so an invalid path or uninitialized
        // target leaves the original dataflow fact unchanged.
        let mut moved = state.clone();
        if !moved.move_at(projections, ty, point, dataflow_problem_ctx).await {
            return false;
        }
        slots.set(root, moved);
        true
    }
}

/// Failure while merging stack-memory dataflow facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackStateError {
    MissingBinding(StackRoot),
    MissingState(StackRoot),
    InvalidStateShape(StackRoot),
}

impl fmt::Display for StackStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBinding(root) => write!(formatter, "missing binding for {root:?}"),
            Self::MissingState(root) => write!(formatter, "missing state for {root:?}"),
            Self::InvalidStateShape(root) => {
                write!(formatter, "incoming states have different component shapes for {root:?}")
            }
        }
    }
}

impl Error for StackStateError {}

/// Dataflow context for stack initialization and move state.
///
/// It stores only each stack root's type. Aggregate structure is queried lazily
/// through the tracked engine when an address projects into that aggregate, so
/// recursive types are never expanded eagerly.
#[derive(Debug, Clone)]
pub struct StackStateProblem {
    engine: TrackedEngine,
    bindings: FxHashMap<StackRoot, Interned<Ty>>,
}

impl StackStateProblem {
    #[must_use]
    pub fn new(engine: TrackedEngine) -> Self { Self { engine, bindings: FxHashMap::default() } }

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
    /// Struct bodies and generic field substitutions are queried through the
    /// tracked engine only for the current projection. Types beneath fields
    /// not selected by the address are never traversed, and no collection of
    /// future layers is built, which keeps recursive types finite.
    ///
    /// Returns `None` when `projection` is invalid for `ty`.
    async fn projection_layer(
        &self,
        ty: &Interned<Ty>,
        projection: Projection,
        place_state: &PlaceState,
    ) -> Option<(Option<BTreeMap<Projection, PlaceState>>, Interned<Ty>)> {
        let Ty::Application(application) = &**ty else {
            return None;
        };

        match (application.view(), projection) {
            (ApplicationView::Tuple(tuple), Projection::Tuple(index)) => {
                let projected = tuple.args().get(index)?.clone();
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
                Some((components, projected))
            }
            (ApplicationView::Struct(struct_ty), Projection::Field(field_id)) => {
                let substitution = struct_ty.create_subst(&self.engine).await;
                let body = self.engine.get_struct_body(struct_ty.symbol_id()).await;
                let field =
                    body.iter().find_map(|(id, field)| (id == field_id).then_some(field))?;
                let projected = field.ty().apply_subst_or_clone(&substitution, &self.engine);
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
                Some((components, projected))
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
            | (ApplicationView::Struct(_), Projection::Tuple(_)) => None,
        }
    }
}

impl JoinLattice<StackStateProblem, StackStateError> for StackState {
    async fn join(
        &mut self,
        other: &Self,
        dataflow_problem_ctx: &StackStateProblem,
    ) -> Result<bool, StackStateError> {
        match (&*self, other) {
            (Self::Unreachable | Self::Reachable(_), Self::Unreachable) => Ok(false),
            (Self::Unreachable, Self::Reachable(_)) => {
                *self = other.clone();
                Ok(true)
            }
            (Self::Reachable(current), Self::Reachable(incoming)) => {
                let mut joined = current.clone();

                // Every reachable fact must describe every stack slot known by
                // the problem. Partial states carry their already-expanded
                // immediate sibling sets, so joining requires no type query.
                for root in dataflow_problem_ctx.bindings.keys() {
                    let left = current.state(*root).ok_or(StackStateError::MissingState(*root))?;
                    let right =
                        incoming.state(*root).ok_or(StackStateError::MissingState(*root))?;
                    let state =
                        left.joined(right).ok_or(StackStateError::InvalidStateShape(*root))?;
                    joined.set(*root, state);
                }

                for root in current.states.keys().chain(incoming.states.keys()) {
                    if !dataflow_problem_ctx.bindings.contains_key(root) {
                        return Err(StackStateError::MissingBinding(*root));
                    }
                }

                let changed = *current != joined;
                if changed {
                    *self = Self::Reachable(joined);
                }
                Ok(changed)
            }
        }
    }
}

impl DataflowProblem for StackStateProblem {
    type JoinLattice = StackState;
    type Error = StackStateError;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<StackState, StackStateError> {
        Ok(StackState::Unreachable)
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<StackState, StackStateError> {
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
    ) -> Result<(), StackStateError> {
        Ok(())
    }

    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut StackState,
    ) -> Result<(), StackStateError> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut StackState,
    ) -> Result<(), StackStateError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
