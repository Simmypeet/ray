//! Backward liveness of the locals of an IR function.
//!
//! A local is **use-live** at a point when some path from that point reads
//! its current value before overwriting it. A local is **drop-live** at a
//! point when its current value is not used again on any path, but is still
//! passed to a `Drop.drop` call inserted by drop elaboration on some path.
//! The two sets are disjoint: a local that is use-live is not also reported
//! as drop-live, since a use keeps everything in its type alive anyway.
//!
//! The borrow checker needs the distinction because a drop keeps fewer
//! regions of the dropped type alive than a use does.
//!
//! Liveness is tracked per local, not per place: using or dropping any part of
//! a local makes the whole local live, as in rustc.

use std::{collections::BTreeSet, convert::Infallible};

use crate::{
    address::{Address, Local},
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, DataflowSolution, Direction, JoinLattice},
    ir_expr::{IRExprKind, load::LoadKind},
    ir_function::IRFunction,
};

/// The locals live at one point of a function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveLocals {
    use_live: BTreeSet<Local>,
    drop_live: BTreeSet<Local>,
}

impl LiveLocals {
    /// Returns whether the current value of `local` may be used later.
    #[must_use]
    pub fn is_use_live(&self, local: Local) -> bool { self.use_live.contains(&local) }

    /// Returns whether the current value of `local` may be dropped later
    /// without being used first.
    #[must_use]
    pub fn is_drop_live(&self, local: Local) -> bool { self.drop_live.contains(&local) }

    /// Returns the use-live locals, in order.
    #[must_use]
    pub fn use_live(&self) -> impl ExactSizeIterator<Item = Local> + '_ {
        self.use_live.iter().copied()
    }

    /// Returns the drop-live locals, in order.
    #[must_use]
    pub fn drop_live(&self) -> impl ExactSizeIterator<Item = Local> + '_ {
        self.drop_live.iter().copied()
    }

    fn use_local(&mut self, local: Local) {
        self.use_live.insert(local);
        self.drop_live.remove(&local);
    }

    fn drop_local(&mut self, local: Local) {
        if !self.use_live.contains(&local) {
            self.drop_live.insert(local);
        }
    }

    fn define_local(&mut self, local: Local) {
        self.use_live.remove(&local);
        self.drop_live.remove(&local);
    }
}

impl<D: Sync + ?Sized> JoinLattice<D, Infallible> for LiveLocals {
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        let before = self.clone();

        // A local used on any path is use-live, which subsumes being dropped
        // on another path.
        self.use_live.extend(other.use_live.iter().copied());
        self.drop_live.extend(other.drop_live.iter().copied());
        self.drop_live.retain(|local| !self.use_live.contains(local));

        Ok(*self != before)
    }
}

/// The backward dataflow problem computing [`LiveLocals`].
#[derive(Debug)]
struct LivenessProblem<'a> {
    function: &'a IRFunction,
}

impl LivenessProblem<'_> {
    /// Moves `state` from just after `instruction` to just before it.
    fn transfer(&self, instruction: &Instruction, state: &mut LiveLocals) {
        match instruction {
            Instruction::Expression(expression_id) => {
                match self.function.get_expression(*expression_id).kind() {
                    // Drop elaboration moves a value out only to drop it.
                    IRExprKind::Load(load) => match load.kind() {
                        LoadKind::Drop => {
                            if let Some(local) = load.address().local() {
                                state.drop_local(local);
                            }
                        }
                        LoadKind::Implicit | LoadKind::Move => use_address(load.address(), state),
                    },

                    IRExprKind::RefOf(ref_of) => use_address(ref_of.address(), state),

                    // Every other operand is an already evaluated expression,
                    // not a place.
                    IRExprKind::Error
                    | IRExprKind::Literal(_)
                    | IRExprKind::Binary(_)
                    | IRExprKind::Call(_)
                    | IRExprKind::Phi(_)
                    | IRExprKind::Perform(_)
                    | IRExprKind::Tuple(_)
                    | IRExprKind::Closure(_)
                    | IRExprKind::Handle(_)
                    | IRExprKind::StructInitialization(_) => {}
                }
            }

            // Overwriting a whole local ends its previous value. A store
            // through a dereference reads the pointer instead, and a store to
            // a part of a local leaves the rest of it live.
            Instruction::Store(store) => {
                let address = store.address();
                let Some(local) = address.local() else {
                    return;
                };

                if address.is_behind_deref() {
                    state.use_local(local);
                } else if address.projections().is_empty() {
                    state.define_local(local);
                }
            }

            // A local does not hold a value outside its scope.
            Instruction::ScopePush(scope_id) | Instruction::ScopePop(scope_id) => {
                for variable_id in self.function.declared_variables(*scope_id) {
                    state.define_local(Local::Variable(variable_id));
                }
            }

            Instruction::ExprDiscard(_) => {}
        }
    }
}

/// Records a read of the place `address`, which uses its local, or the
/// pointer it dereferences.
fn use_address(address: &Address, state: &mut LiveLocals) {
    if let Some(local) = address.local() {
        state.use_local(local);
    }
}

impl DataflowProblem for LivenessProblem<'_> {
    type JoinLattice = LiveLocals;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Backward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<LiveLocals, Infallible> {
        Ok(LiveLocals::default())
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<LiveLocals, Infallible> {
        Ok(LiveLocals::default())
    }

    async fn transfer_instruction(
        &mut self,
        _point: Point,
        instruction: &Instruction,
        state: &mut LiveLocals,
    ) -> Result<(), Infallible> {
        self.transfer(instruction, state);
        Ok(())
    }

    // Terminators only read evaluated expressions.
    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut LiveLocals,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut LiveLocals,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}

/// The solved liveness of the locals of one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Liveness {
    solution: DataflowSolution<LiveLocals>,
}

impl Liveness {
    /// Computes the liveness of every local of `function`.
    pub async fn compute(function: &IRFunction) -> Self {
        let mut problem = LivenessProblem { function };
        let Ok(solution) = function.solve_dataflow(&mut problem).await;
        Self { solution }
    }

    /// Returns the locals live just before the instruction at `point`, or
    /// `None` when its block is unreachable.
    ///
    /// A point one past the last instruction of a block stands for its
    /// terminator, where the block's exit facts hold.
    ///
    /// # Panics
    ///
    /// Panics if `point` lies past the end of its block.
    #[must_use]
    pub fn live_before(&self, function: &IRFunction, point: Point) -> Option<LiveLocals> {
        let mut state = self.solution.block_exit(point.block_id())?.clone();
        let problem = LivenessProblem { function };

        let instructions = function.block_instructions(point.block_id());
        assert!(point.instruction_idx() <= instructions.len(), "point lies past its block");

        for instruction in instructions[point.instruction_idx()..].iter().rev() {
            problem.transfer(instruction, &mut state);
        }
        Some(state)
    }

    /// Returns the locals live on entry to `block_id`, or `None` when the
    /// block is unreachable.
    #[must_use]
    pub fn block_entry(&self, block_id: BlockID) -> Option<&LiveLocals> {
        self.solution.block_entry(block_id)
    }
}

#[cfg(test)]
mod tests;
