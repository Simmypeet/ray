//! Backward liveness of the expression values of an IR function.
//!
//! Every [`IRExprID`] is defined by exactly one [`Instruction::Expression`]
//! and consumed at most once, by one of:
//!
//! - an operand of another expression;
//! - the value of a [`Instruction::Store`];
//! - the condition of a [`Terminator::Conditional`] or the value of a
//!   [`Terminator::Return`];
//! - an incoming value of a phi, consumed on the edge from its predecessor;
//! - an [`Instruction::ExprDiscard`], which drops it.
//!
//! An expression is **use-live** at a point when some path from that point
//! reaches its use, and **drop-live** when it only reaches its discard. On a
//! well-formed function, whose every path consumes each defined value
//! exactly once, this is exactly the set of values defined and not yet
//! consumed. Where a path leaves a value unconsumed, such as an early `return`
//! in the middle of an evaluation, the value is dead on that path, since it is
//! never read again.

use std::convert::Infallible;

use super::LiveSet;
use crate::{
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, DataflowSolution, Direction},
    ir_expr::IRExprID,
    ir_function::IRFunction,
};

/// The expression values live at one point of a function.
pub type LiveExprs = LiveSet<IRExprID>;

/// The backward dataflow problem computing [`LiveExprs`].
#[derive(Debug)]
struct ExprLivenessProblem<'a> {
    function: &'a IRFunction,
}

impl ExprLivenessProblem<'_> {
    /// Moves `state` from just after `instruction` to just before it.
    fn transfer(&self, instruction: &Instruction, state: &mut LiveExprs) {
        match instruction {
            // The value is defined here, after its operands are consumed.
            Instruction::Expression(expression_id) => {
                state.mark_defined(*expression_id);

                // A phi's incoming values are consumed on the edges into its
                // block instead; see `phi_operands`.
                let kind = self.function.get_expression(*expression_id).kind();
                if kind.as_phi().is_none() {
                    for operand in kind.operands() {
                        state.mark_used(operand);
                    }
                }
            }

            Instruction::ExprDiscard(discard) => state.mark_dropped(discard.expression()),

            Instruction::Store(store) => state.mark_used(store.expression()),

            // A drop reads a place, not an evaluated expression.
            Instruction::ScopePush(_) | Instruction::ScopePop(_) | Instruction::AddressDrop(_) => {}
        }
    }

    /// Moves `state` from just after `terminator` to just before it.
    fn transfer_across_terminator(terminator: &Terminator, state: &mut LiveExprs) {
        match terminator {
            Terminator::Conditional(conditional) => state.mark_used(conditional.condition()),
            Terminator::Return(Some(value)) => state.mark_used(*value),
            Terminator::Return(None) | Terminator::Jump(_) => {}
        }
    }

    /// Returns the phi operands consumed when control flows along `edge`:
    /// the values the phis of its target take from its source.
    fn phi_operands(&self, edge: &ControlFlowEdge) -> impl Iterator<Item = IRExprID> + '_ {
        let source = edge.source();
        self.function.block_instructions(edge.target()).iter().filter_map(move |instruction| {
            let Instruction::Expression(expression_id) = instruction else {
                return None;
            };
            self.function.get_expression(*expression_id).kind().as_phi()?.value_from(source)
        })
    }
}

impl DataflowProblem for ExprLivenessProblem<'_> {
    type JoinLattice = LiveExprs;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Backward;

    // A phi operand is consumed on one incoming edge only, so it must not be
    // live on the other edges into the same block.
    const EDGE_SENSITIVE: bool = true;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<LiveExprs, Infallible> {
        Ok(LiveExprs::default())
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<LiveExprs, Infallible> {
        Ok(LiveExprs::default())
    }

    async fn transfer_instruction(
        &mut self,
        _point: Point,
        instruction: &Instruction,
        state: &mut LiveExprs,
    ) -> Result<(), Infallible> {
        self.transfer(instruction, state);
        Ok(())
    }

    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        terminator: &Terminator,
        state: &mut LiveExprs,
    ) -> Result<(), Infallible> {
        Self::transfer_across_terminator(terminator, state);
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        edge: &ControlFlowEdge,
        state: &mut LiveExprs,
    ) -> Result<(), Infallible> {
        for operand in self.phi_operands(edge) {
            state.mark_used(operand);
        }
        Ok(())
    }
}

/// The solved liveness of the expression values of one function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExprLiveness {
    solution: DataflowSolution<LiveExprs>,
}

impl ExprLiveness {
    /// Computes the liveness of every expression value of `function`.
    pub async fn compute(function: &IRFunction) -> Self {
        let mut problem = ExprLivenessProblem { function };
        let Ok(solution) = function.solve_dataflow(&mut problem).await;
        Self { solution }
    }

    /// Returns the expression values live just before the instruction at
    /// `point`, or `None` when its block is unreachable.
    ///
    /// A point one past the last instruction of a block stands for its
    /// terminator, so the values the terminator consumes are live there.
    ///
    /// # Panics
    ///
    /// Panics if `point` lies past the end of its block.
    #[must_use]
    pub fn live_before(&self, function: &IRFunction, point: Point) -> Option<LiveExprs> {
        let problem = ExprLivenessProblem { function };

        // TODO: find a way to avoid cloning the state and replay here.
        let mut state = self.solution.block_exit(point.block_id())?.clone();

        // Replay the block backward from its exit, terminator first.
        if let Some(terminator) = function.block_terminator(point.block_id()) {
            ExprLivenessProblem::transfer_across_terminator(terminator, &mut state);
        }

        let instructions = function.block_instructions(point.block_id());
        assert!(point.instruction_idx() <= instructions.len(), "point lies past its block");

        for instruction in instructions[point.instruction_idx()..].iter().rev() {
            problem.transfer(instruction, &mut state);
        }
        Some(state)
    }

    /// Returns the expression values live on entry to `block_id`, or `None`
    /// when the block is unreachable.
    ///
    /// The incoming values of the block's phis are not live here: each was
    /// consumed on the edge from its predecessor.
    #[must_use]
    pub fn block_entry(&self, block_id: BlockID) -> Option<&LiveExprs> {
        self.solution.block_entry(block_id)
    }
}

#[cfg(test)]
mod tests;
