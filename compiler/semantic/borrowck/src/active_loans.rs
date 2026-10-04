//! The loans active at each point of an IR function: those borrowed on some
//! path to the point that nothing has ended since. A forward gen/kill dataflow,
//! where a loan is also killed once it is no longer live.

use std::convert::Infallible;

use rayc_hash::FxHashSet;
use rayc_ir::{
    address::{Address, Local},
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, DataflowSolution, Direction, JoinLattice},
    ir_function::IRFunction,
    scope::ScopeID,
};

use crate::{
    constraint::{LoanID, LocalizedConstraints},
    live_loans::LiveLoans,
};

/// The loans active at one point of an IR function.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActiveLoans {
    loans: FxHashSet<LoanID>,
}

impl ActiveLoans {
    /// Returns whether `loan` is active.
    #[must_use]
    pub fn contains(&self, loan: LoanID) -> bool { self.loans.contains(&loan) }

    /// Iterates over the active loans, in unspecified order.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = LoanID> + '_ { self.loans.iter().copied() }
}

impl<D: Sync + ?Sized> JoinLattice<D, Infallible> for ActiveLoans {
    async fn join(&mut self, other: &Self, _dataflow_problem_ctx: &D) -> Result<bool, Infallible> {
        // A loan active on any incoming path is active.
        let len = self.loans.len();
        self.loans.extend(other.loans.iter().copied());
        Ok(self.loans.len() != len)
    }
}

/// The forward dataflow problem computing [`ActiveLoans`].
#[derive(Debug)]
struct ActiveLoansProblem<'a> {
    function: &'a IRFunction,
    constraints: &'a LocalizedConstraints,
    live_loans: &'a LiveLoans,
}

impl ActiveLoansProblem<'_> {
    /// Kills the loans that are not live at `point`, before the instruction
    /// there takes effect.
    fn kill_dead_loans(&self, point: Point, state: &mut ActiveLoans) {
        state.loans.retain(|&loan| self.live_loans.is_live(loan, point));
    }

    /// Applies the effect of `instruction` on the active loans.
    fn apply_instruction(&self, instruction: &Instruction, state: &mut ActiveLoans) {
        match instruction {
            Instruction::Store(store) => self.kill_overwritten_loans(store.address(), state),
            Instruction::ScopePop(scope_id) => self.kill_scope_loans(*scope_id, state),

            // Only a borrow issues a loan, and every borrow is an expression.
            Instruction::Expression(expression_id) => {
                if let Some(loan) = self.constraints.loan_id_of_ref_of(*expression_id) {
                    state.loans.insert(loan);
                }
            }

            // A drop reads its place rather than overwriting it, and a new
            // scope has no loans yet.
            Instruction::ScopePush(_)
            | Instruction::ExprDiscard(_)
            | Instruction::AddressDrop(_) => {}
        }
    }

    /// Kills the loans of every place within `overwritten`, whose previous
    /// values are gone.
    fn kill_overwritten_loans(&self, overwritten: &Address, state: &mut ActiveLoans) {
        let Some(local) = overwritten.local() else {
            return;
        };

        for loan in self.constraints.loans_of_local(local) {
            if overwritten.contains(self.constraints.get_loan(loan).address()) {
                state.loans.remove(&loan);
            }
        }
    }

    /// Kills the loans of the places in the variables declared in
    /// `scope_id`, whose storage ends with the scope.
    fn kill_scope_loans(&self, scope_id: ScopeID, state: &mut ActiveLoans) {
        for variable_id in self.function.declared_variables(scope_id) {
            for loan in self.constraints.loans_of_local(Local::Variable(variable_id)) {
                state.loans.remove(&loan);
            }
        }
    }

    /// Returns the point of the terminator of `block_id`, one past its last
    /// instruction.
    fn terminator_point(&self, block_id: BlockID) -> Point {
        let instruction_idx = self.function.block_instructions(block_id).len();
        Point::builder().block_id(block_id).instruction_idx(instruction_idx).build()
    }
}

impl DataflowProblem for ActiveLoansProblem<'_> {
    type JoinLattice = ActiveLoans;
    type Error = Infallible;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<ActiveLoans, Infallible> {
        Ok(ActiveLoans::default())
    }

    // No loan is active on entry to the function.
    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<ActiveLoans, Infallible> {
        Ok(ActiveLoans::default())
    }

    async fn transfer_instruction(
        &mut self,
        point: Point,
        instruction: &Instruction,
        state: &mut ActiveLoans,
    ) -> Result<(), Infallible> {
        self.kill_dead_loans(point, state);
        self.apply_instruction(instruction, state);
        Ok(())
    }

    // A terminator only reads evaluated expressions, so it has no effect of
    // its own on loans.
    async fn transfer_terminator(
        &mut self,
        block_id: BlockID,
        _terminator: &Terminator,
        state: &mut ActiveLoans,
    ) -> Result<(), Infallible> {
        self.kill_dead_loans(self.terminator_point(block_id), state);
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut ActiveLoans,
    ) -> Result<(), Infallible> {
        Ok(())
    }
}

/// The solved active loans of one IR function.
#[derive(Debug)]
pub struct LoanActivity<'a> {
    problem: ActiveLoansProblem<'a>,
    solution: DataflowSolution<ActiveLoans>,
}

impl<'a> LoanActivity<'a> {
    /// Computes the loans active at each point of `function`.
    pub async fn compute(
        function: &'a IRFunction,
        constraints: &'a LocalizedConstraints,
        live_loans: &'a LiveLoans,
    ) -> Self {
        let mut problem = ActiveLoansProblem { function, constraints, live_loans };
        let Ok(solution) = function.solve_dataflow(&mut problem).await;
        Self { problem, solution }
    }

    /// Calls `visit` at each point of `block_id` with the instruction there, or
    /// `None` for the terminator, and the loans active just before it. Does
    /// nothing for an unreachable block.
    pub async fn visit_block(
        &self,
        block_id: BlockID,
        mut visit: impl AsyncFnMut(Point, Option<&Instruction>, &ActiveLoans),
    ) {
        let Some(entry) = self.solution.block_entry(block_id) else {
            return;
        };

        let problem = &self.problem;
        let mut state = entry.clone();
        for (point, instruction) in problem.function.block_instructions_with_points(block_id) {
            problem.kill_dead_loans(point, &mut state);
            visit(point, Some(instruction), &state).await;
            problem.apply_instruction(instruction, &mut state);
        }

        let terminator = problem.terminator_point(block_id);
        problem.kill_dead_loans(terminator, &mut state);
        visit(terminator, None, &state).await;
    }
}

#[cfg(test)]
mod tests;
