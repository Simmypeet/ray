use std::collections::{BTreeMap, BTreeSet};

use qbice::storage::intern::Interned;
use rayc_ir::{address::Projection, cfg::Point};
use rayc_type::ty::Ty;

use crate::StackStateProblem;

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

    /// Returns the state of the immediate component selected by `projection`.
    ///
    /// A uniform state describes every component, so it is returned as is.
    pub(crate) fn component(&self, projection: Projection) -> &Self {
        match self {
            Self::Uniform(_) => self,
            Self::Partial(components) => components
                .get(&projection)
                .expect("the type-checked projection must exist in the place state"),
        }
    }

    /// Visits every uninitialized leaf beneath this place.
    pub(crate) fn visit_uninitialized(&self, visitor: &mut impl FnMut(&MoveHistory)) {
        match self {
            Self::Uniform(PossibleStates::Initialized) => {}
            Self::Uniform(PossibleStates::Uninitialized(history)) => visitor(history),
            Self::Partial(components) => {
                for component in components.values() {
                    component.visit_uninitialized(visitor);
                }
            }
        }
    }

    pub(crate) async fn move_at(
        &mut self,
        projections: &[Projection],
        ty: Interned<Ty>,
        point: Point,
        dataflow_problem_ctx: &StackStateProblem<'_>,
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

    pub(crate) async fn restore_at(
        &mut self,
        projections: &[Projection],
        ty: Interned<Ty>,
        dataflow_problem_ctx: &StackStateProblem<'_>,
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
        dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> Option<&'a mut Self> {
        let mut selected = self;

        for projection in projections {
            if !update.can_traverse(selected) {
                return None;
            }

            // Resolve and expand only the layer being traversed. This keeps
            // recursive types finite and preserves every sibling's state.
            let uniform = match &*selected {
                Self::Uniform(state) => Some(state),
                Self::Partial(_) => None,
            };
            let (components, projected_ty) =
                dataflow_problem_ctx.projection_layer(&ty, *projection, uniform).await;
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

    pub(crate) fn join_in_place(&mut self, other: &Self) -> bool {
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
