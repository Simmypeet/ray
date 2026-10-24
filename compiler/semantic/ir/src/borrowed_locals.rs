//! Forward analysis of the locals of an IR function whose storage may be
//! borrowed.
//!
//! A local is **maybe borrowed** at a point when some path to that point
//! evaluates a [`RefOf`] of a place in its own storage, and the local's
//! storage has not ended since. A borrow through a dereference borrows the
//! memory behind the pointer, not the local holding it, so it does not count.
//!
//! The storage of a variable ends at the [`Instruction::ScopePush`] and the
//! [`Instruction::ScopePop`] of the scope that declares it. Parameters keep
//! their storage for the whole function, so nothing ends their borrows.
//! Captures live in the environment of the nested function rather than in its
//! frame, so they are never tracked.
//!
//! The analysis is conservative, like rustc's `MaybeBorrowedLocals`: a borrow
//! that is no longer used still keeps its local borrowed until its storage
//! ends. Unlike the borrow checker's loans, it reads nothing but the IR, so
//! anything computed before borrow checking can depend on it.
//!
//! [`RefOf`]: crate::ir_expr::ref_of::RefOf

use std::{collections::BTreeSet, convert::Infallible};

use crate::{
    address::Local,
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, DataflowSolution, Direction, JoinLattice},
    ir_expr::IRExprKind,
    ir_function::IRFunction,
};

/// The locals whose storage may be borrowed at one point of a function.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BorrowedLocals {
    locals: BTreeSet<Local>,
}

impl BorrowedLocals {
    /// Returns whether the storage of `local` may be borrowed.
    #[must_use]
    pub fn contains(&self, local: Local) -> bool { self.locals.contains(&local) }

    /// Returns the locals that may be borrowed, in order.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Local> + '_ { self.locals.iter().copied() }
}

impl<D: Sync + ?Sized> JoinLattice<D, Infallible> for BorrowedLocals {
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        let len = self.locals.len();
        self.locals.extend(other.locals.iter().copied());
        Ok(self.locals.len() != len)
    }
}

/// The forward dataflow problem computing [`BorrowedLocals`].
#[derive(Debug)]
struct BorrowedLocalsProblem<'a> {
    function: &'a IRFunction,
}

impl BorrowedLocalsProblem<'_> {
    /// Moves `state` from just before `instruction` to just after it.
    fn transfer(&self, instruction: &Instruction, state: &mut BorrowedLocals) {
        match instruction {
            Instruction::Expression(expression_id) => {
                let IRExprKind::RefOf(ref_of) = self.function.get_expression(*expression_id).kind()
                else {
                    return;
                };

                match ref_of.address().direct_local() {
                    Some(Local::Capture(_)) | None => {}
                    Some(local) => {
                        state.locals.insert(local);
                    }
                }
            }

            // A variable has no storage outside its scope, so there is
            // nothing left to borrow.
            Instruction::ScopePush(scope_id) | Instruction::ScopePop(scope_id) => {
                for variable_id in self.function.declared_variables(*scope_id) {
                    state.locals.remove(&Local::Variable(variable_id));
                }
            }

            // A store or a drop may end the value of a local, but not its
            // storage, which a borrow still points to.
            Instruction::Store(_) | Instruction::AddressDrop(_) | Instruction::ExprDiscard(_) => {}
        }
    }
}

impl DataflowProblem for BorrowedLocalsProblem<'_> {
    type JoinLattice = BorrowedLocals;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<BorrowedLocals, Infallible> {
        Ok(BorrowedLocals::default())
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<BorrowedLocals, Infallible> {
        Ok(BorrowedLocals::default())
    }

    async fn transfer_instruction(
        &mut self,
        _point: Point,
        instruction: &Instruction,
        state: &mut BorrowedLocals,
    ) -> Result<(), Infallible> {
        self.transfer(instruction, state);
        Ok(())
    }

    // A terminator evaluates no `RefOf`: a borrow is always an instruction.
    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut BorrowedLocals,
    ) -> Result<(), Infallible> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut BorrowedLocals,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}

/// The solved maybe-borrowed locals of one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaybeBorrowedLocals {
    solution: DataflowSolution<BorrowedLocals>,
}

impl MaybeBorrowedLocals {
    /// Computes the maybe-borrowed locals of `function`.
    pub async fn compute(function: &IRFunction) -> Self {
        let mut problem = BorrowedLocalsProblem { function };
        let Ok(solution) = function.solve_dataflow(&mut problem).await;
        Self { solution }
    }

    /// Returns the locals that may be borrowed just before the instruction at
    /// `point`, or `None` when its block is unreachable.
    ///
    /// A point one past the last instruction of a block stands for its
    /// terminator, where the block's exit facts hold.
    ///
    /// # Panics
    ///
    /// Panics if `point` lies past the end of its block.
    #[must_use]
    pub fn borrowed_before(&self, function: &IRFunction, point: Point) -> Option<BorrowedLocals> {
        let mut state = self.solution.block_entry(point.block_id())?.clone();
        let problem = BorrowedLocalsProblem { function };

        let instructions = function.block_instructions(point.block_id());
        assert!(point.instruction_idx() <= instructions.len(), "point lies past its block");

        for instruction in &instructions[..point.instruction_idx()] {
            problem.transfer(instruction, &mut state);
        }
        Some(state)
    }

    /// Returns the locals that may be borrowed at the exit of `block_id`, or
    /// `None` when the block is unreachable.
    #[must_use]
    pub fn block_exit(&self, block_id: BlockID) -> Option<&BorrowedLocals> {
        self.solution.block_exit(block_id)
    }
}

#[cfg(test)]
mod tests;
