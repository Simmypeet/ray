use std::collections::BTreeMap;

use rayc_hash::FxHashMap;
use rayc_ir::{
    address::{Address, Local, Projection},
    cfg::Point,
    ir_expr::IRExprID,
    scope::ScopeID,
};

use crate::{PlaceState, StackStateProblem};

/// Initialization state of the stack allocations in one function, and the
/// evaluated values it still owns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StackSlots {
    states: FxHashMap<Local, PlaceState>,

    /// The active scopes, innermost last.
    scopes: Vec<ScopeID>,

    /// The values evaluated but not consumed yet, such as the earlier
    /// arguments of a call whose later arguments are still being evaluated.
    /// Each is owned by the scope that drops it if its consumer is never
    /// reached: the innermost scope active when it was evaluated, or an
    /// enclosing scope it flowed out to.
    pending: BTreeMap<IRExprID, ScopeID>,
}

impl StackSlots {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Inserts or replaces the state of a stack allocation.
    pub fn set(&mut self, root: Local, state: PlaceState) {
        let _ = self.states.insert(root, state);
    }

    /// Returns the state of a stack allocation.
    #[must_use]
    pub fn state(&self, root: Local) -> Option<&PlaceState> { self.states.get(&root) }

    pub(crate) fn state_mut(&mut self, root: Local) -> Option<&mut PlaceState> {
        self.states.get_mut(&root)
    }

    pub(crate) fn remove(&mut self, root: Local) { self.states.remove(&root); }

    /// Begins `scope_id` inside the innermost active scope.
    pub(crate) fn push_scope(&mut self, scope_id: ScopeID) { self.scopes.push(scope_id); }

    /// Ends `scope_id`, the innermost active scope.
    ///
    /// The values it owns which `is_used_later` are handed to the enclosing
    /// scope, like the value of a block flowing out of the block. The others
    /// are never consumed, so they are dropped as the scope ends and are no
    /// longer owned.
    pub(crate) fn pop_scope(
        &mut self,
        scope_id: ScopeID,
        mut is_used_later: impl FnMut(IRExprID) -> bool,
    ) {
        let popped = self.scopes.pop();
        assert_eq!(popped, Some(scope_id), "only the innermost active scope can end");

        // Nothing encloses the root scope. The only value which outlives it
        // is the returned one, which the return consumes right after, so it
        // keeps the root as its owner.
        let enclosing = self.scopes.last().copied().unwrap_or(scope_id);
        self.pending.retain(|value, owner| {
            if *owner != scope_id {
                return true;
            }

            let used_later = is_used_later(*value);
            if used_later {
                *owner = enclosing;
            }
            used_later
        });
    }

    /// Records that `value` was just evaluated, owned by the innermost active
    /// scope.
    pub(crate) fn define_value(&mut self, value: IRExprID) {
        let owner = *self.scopes.last().expect("a value is evaluated inside a scope");
        self.pending.insert(value, owner);
    }

    /// Records that `value` was consumed, so it is no longer owned here.
    pub(crate) fn consume_value(&mut self, value: IRExprID) { self.pending.remove(&value); }

    /// Returns whether `value` was evaluated and is not consumed yet.
    #[must_use]
    pub fn is_pending(&self, value: IRExprID) -> bool { self.pending.contains_key(&value) }

    /// Iterates over the values evaluated and not consumed yet, newest first.
    pub fn pending_values(&self) -> impl Iterator<Item = IRExprID> + '_ {
        self.pending.keys().rev().copied()
    }

    /// Iterates over the values not consumed yet which `scope_id` owns,
    /// newest first.
    pub fn values_owned_by(&self, scope_id: ScopeID) -> impl Iterator<Item = IRExprID> + '_ {
        self.pending
            .iter()
            .rev()
            .filter(move |(_, owner)| **owner == scope_id)
            .map(|(value, _)| *value)
    }

    pub(crate) fn join_in_place(&mut self, incoming: &Self) -> bool {
        assert_eq!(
            self.states.len(),
            incoming.states.len(),
            "incoming stack states have different live allocation counts",
        );
        assert_eq!(self.scopes, incoming.scopes, "incoming stack states have different scopes");

        let pending_count = self.pending.len();
        self.pending.retain(|value, owner| {
            if let Some(other_owner) = incoming.pending.get(value) {
                assert_eq!(
                    owner, other_owner,
                    "incoming stack states have different owners for the same value",
                );
                return true;
            }

            false
        });
        let mut changed = pending_count != self.pending.len();

        // Scope verification guarantees matching live roots and place shapes,
        // so update each state directly in place.
        for (root, current) in &mut self.states {
            let incoming = incoming.state(*root).expect("incoming stack state is missing a root");
            changed |= current.join_in_place(incoming);
        }
        changed
    }
}

/// Memory-checker facts at a point in the control-flow graph.
///
/// Only places in a local's own storage are tracked, the ones selected by an
/// address with a [`Address::direct_local`]. Memory reached through a pointer
/// is not owned by the stack frame, so tracking stops at the first
/// dereference: the pointer itself is tracked through
/// [`Address::deref_base`], but nothing beyond it is.
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
        dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> bool {
        let Some(root) = address.direct_local() else {
            return false;
        };

        self.move_place(root, address.projections(), point, dataflow_problem_ctx).await
    }

    /// Returns the current state of the place selected by `address`.
    ///
    /// Untracked addresses have no stack state. Traversal stops at the first
    /// uniform state because it describes every descendant of that place.
    #[must_use]
    pub fn place_state(&self, address: &Address) -> Option<&PlaceState> {
        let root = address.direct_local()?;
        let Self::Reachable(slots) = self else {
            return None;
        };

        let mut selected = slots.state(root)?;
        for projection in address.projections() {
            match selected {
                PlaceState::Uniform(_) => return Some(selected),
                PlaceState::Partial(components) => {
                    selected = components
                        .get(projection)
                        .expect("the type-checked projection must exist in the place state");
                }
            }
        }

        Some(selected)
    }

    /// Moves from a stack root and projection path.
    ///
    /// This lower-level form is useful when an analysis already decomposed an
    /// [`Address`] into its stack root and projections.
    pub async fn move_place(
        &mut self,
        root: Local,
        projections: &[Projection],
        point: Point,
        dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> bool {
        let Self::Reachable(slots) = self else {
            return false;
        };
        let ty = dataflow_problem_ctx.binding_type(root).await;
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
        dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> bool {
        let Some(root) = address.direct_local() else {
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
        root: Local,
        projections: &[Projection],
        dataflow_problem_ctx: &StackStateProblem<'_>,
    ) -> bool {
        let Self::Reachable(slots) = self else {
            return false;
        };
        let ty = dataflow_problem_ctx.binding_type(root).await;
        let state = slots.state_mut(root).expect("tracked stack root must have a state");

        state.restore_at(projections, ty, dataflow_problem_ctx).await
    }
}
