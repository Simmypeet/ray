use std::{error::Error, fmt};

use rayc_ir::{
    cfg::{BlockID, ControlFlowEdge, Instruction, Point, Terminator},
    dataflow::{DataflowProblem, Direction, JoinLattice},
    ir_function::{FunctionID, IRFunctionMap},
    scope::ScopeID,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScopeStack {
    Unreachable,
    Active(Vec<ScopeID>),
}

impl ScopeStack {
    const fn active() -> Self { Self::Active(Vec::new()) }
}

impl JoinLattice<ScopeStackProblem, ScopeStackError> for ScopeStack {
    async fn join(
        &mut self,
        other: &Self,
        _dataflow_problem_ctx: &ScopeStackProblem,
    ) -> Result<bool, ScopeStackError> {
        match (&*self, other) {
            (Self::Unreachable | Self::Active(_), Self::Unreachable) => Ok(false),

            (Self::Unreachable, Self::Active(scopes)) => {
                *self = Self::Active(scopes.clone());
                Ok(true)
            }

            (Self::Active(current), Self::Active(incoming)) if current == incoming => Ok(false),
            (Self::Active(current), Self::Active(incoming)) => {
                Err(ScopeStackError::MergeConflict {
                    first: incoming.clone(),
                    second: current.clone(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScopeStackError {
    InvalidPop { point: Point, expected: Option<ScopeID>, actual: ScopeID },
    MergeConflict { first: Vec<ScopeID>, second: Vec<ScopeID> },
}

#[derive(Debug, Default)]
struct ScopeStackProblem;

impl DataflowProblem for ScopeStackProblem {
    type JoinLattice = ScopeStack;
    type Error = ScopeStackError;

    const DIRECTION: Direction = Direction::Forward;
    const EDGE_SENSITIVE: bool = false;

    async fn bottom(&mut self, _block_id: BlockID) -> Result<ScopeStack, ScopeStackError> {
        Ok(ScopeStack::Unreachable)
    }

    async fn boundary_facts(&mut self, _block_id: BlockID) -> Result<ScopeStack, ScopeStackError> {
        Ok(ScopeStack::active())
    }

    async fn transfer_instruction(
        &mut self,
        point: Point,
        instruction: &Instruction,
        state: &mut ScopeStack,
    ) -> Result<(), ScopeStackError> {
        let ScopeStack::Active(scopes) = state else {
            return Ok(());
        };

        // Track scope lifetime operations while leaving all value-producing
        // instructions irrelevant to this verification.
        match instruction {
            Instruction::ScopePush(scope_id) => scopes.push(*scope_id),
            Instruction::ScopePop(scope_id) => {
                let active_scope = scopes.last().copied();
                if active_scope != Some(*scope_id) {
                    return Err(ScopeStackError::InvalidPop {
                        point,
                        expected: active_scope,
                        actual: *scope_id,
                    });
                }
                scopes.pop();
            }
            Instruction::Expression(_)
            | Instruction::ExprDiscard(_)
            | Instruction::AddressDrop(_)
            | Instruction::Store(_) => {}
        }

        Ok(())
    }

    async fn transfer_terminator(
        &mut self,
        _block_id: BlockID,
        _terminator: &Terminator,
        _state: &mut ScopeStack,
    ) -> Result<(), ScopeStackError> {
        Ok(())
    }

    async fn transfer_edge(
        &mut self,
        _edge: &ControlFlowEdge,
        _state: &mut ScopeStack,
    ) -> Result<(), ScopeStackError> {
        Ok(())
    }
}

/// Describes a violated scope lifetime invariant in finalized IR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationError {
    function_id: FunctionID,
    kind: VerificationErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerificationErrorKind {
    InvalidPop { point: Point, expected: Option<ScopeID>, actual: ScopeID },
    MergeConflict { first: Vec<ScopeID>, second: Vec<ScopeID> },
    UnclosedScopes { block_id: BlockID, scopes: Vec<ScopeID> },
}

impl fmt::Display for VerificationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "function {:?}: ", self.function_id)?;
        match &self.kind {
            VerificationErrorKind::InvalidPop { point, expected, actual } => write!(
                formatter,
                "scope pop at block {:?}, instruction {} popped {:?}, but the active scope was \
                 {:?}",
                point.block_id(),
                point.instruction_idx(),
                actual,
                expected,
            ),
            VerificationErrorKind::MergeConflict { first, second } => write!(
                formatter,
                "control-flow merge has different active scope stacks: {first:?} and {second:?}",
            ),
            VerificationErrorKind::UnclosedScopes { block_id, scopes } => {
                write!(formatter, "return from block {block_id:?} leaves active scopes {scopes:?}",)
            }
        }
    }
}

impl Error for VerificationError {}

/// Verifies scope push/pop invariants in every finalized IR function.
pub async fn verify(functions: &IRFunctionMap) -> Result<(), VerificationError> {
    for (function_id, function) in functions.functions() {
        let solution =
            function.solve_dataflow(&mut ScopeStackProblem).await.map_err(|error| match error {
                ScopeStackError::InvalidPop { point, expected, actual } => VerificationError {
                    function_id,
                    kind: VerificationErrorKind::InvalidPop { point, expected, actual },
                },
                ScopeStackError::MergeConflict { first, second } => VerificationError {
                    function_id,
                    kind: VerificationErrorKind::MergeConflict { first, second },
                },
            })?;

        // Every path that leaves the function must close all scopes. Infinite
        // paths have no return boundary and are checked by merges instead.
        for block_id in solution.reachable_blocks() {
            if !matches!(function.block_terminator(block_id), Some(Terminator::Return(_))) {
                continue;
            }
            if let Some(ScopeStack::Active(scopes)) = solution.block_exit(block_id)
                && !scopes.is_empty()
            {
                return Err(VerificationError {
                    function_id,
                    kind: VerificationErrorKind::UnclosedScopes {
                        block_id,
                        scopes: scopes.clone(),
                    },
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use rayc_arena::ID;
    use rayc_ir::{
        cfg::{Cfg, Conditional, Terminator},
        ir_expr::IRExpr,
        scope::Scope,
    };

    use super::{ScopeStackProblem, VerificationErrorKind};

    async fn verify_cfg(cfg: &Cfg) -> Result<(), VerificationErrorKind> {
        let solution = rayc_ir::dataflow::solve(&mut ScopeStackProblem, cfg).await.map_err(
            |error| match error {
                super::ScopeStackError::InvalidPop { point, expected, actual } => {
                    VerificationErrorKind::InvalidPop { point, expected, actual }
                }
                super::ScopeStackError::MergeConflict { first, second } => {
                    VerificationErrorKind::MergeConflict { first, second }
                }
            },
        )?;

        for block_id in solution.reachable_blocks() {
            if matches!(cfg.terminator(block_id), Some(Terminator::Return(_)))
                && let Some(super::ScopeStack::Active(scopes)) = solution.block_exit(block_id)
                && !scopes.is_empty()
            {
                return Err(VerificationErrorKind::UnclosedScopes {
                    block_id,
                    scopes: scopes.clone(),
                });
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn rejects_a_pop_that_is_not_the_active_scope() {
        let mut cfg = Cfg::new();
        let entry = cfg.entry_block();
        cfg.push_scope_push_instruction(entry, ID::<Scope>::new(0));
        cfg.push_scope_pop_instruction(entry, ID::<Scope>::new(1));
        cfg.set_terminator(entry, Terminator::Return(None));

        assert!(matches!(verify_cfg(&cfg).await, Err(VerificationErrorKind::InvalidPop { .. })));
    }

    #[tokio::test]
    async fn rejects_different_scope_stacks_at_a_merge() {
        let mut cfg = Cfg::new();
        let entry = cfg.entry_block();
        let then_block = cfg.create_block();
        let else_block = cfg.create_block();
        let merge_block = cfg.create_block();
        cfg.set_terminator(
            entry,
            Terminator::Conditional(Conditional::new(ID::<IRExpr>::new(0), then_block, else_block)),
        );
        cfg.push_scope_push_instruction(then_block, ID::<Scope>::new(0));
        cfg.set_terminator(then_block, Terminator::Jump(merge_block));
        cfg.set_terminator(else_block, Terminator::Jump(merge_block));
        cfg.set_terminator(merge_block, Terminator::Return(None));

        // Which stack is reported first depends on the order the branches
        // are processed in, which is not part of the contract.
        let Err(VerificationErrorKind::MergeConflict { first, second }) = verify_cfg(&cfg).await
        else {
            panic!("differing scope stacks at a merge should be rejected");
        };
        let mut stacks = [first, second];
        stacks.sort();
        assert_eq!(stacks, [Vec::new(), vec![ID::<Scope>::new(0)]]);
    }

    #[tokio::test]
    async fn rejects_active_scopes_at_a_return() {
        let mut cfg = Cfg::new();
        let entry = cfg.entry_block();
        cfg.push_scope_push_instruction(entry, ID::<Scope>::new(0));
        cfg.set_terminator(entry, Terminator::Return(None));

        assert_eq!(
            verify_cfg(&cfg).await,
            Err(VerificationErrorKind::UnclosedScopes {
                block_id: entry,
                scopes: vec![ID::<Scope>::new(0)],
            })
        );
    }
}
