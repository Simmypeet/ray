use bon::Builder;
use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_symbol::GlobalSymbolID;

use super::BlockID;
use crate::ir_expr::IRExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Conditional {
    condition: IRExprID,
    pub(super) true_block: BlockID,
    pub(super) false_block: BlockID,
}

impl Conditional {
    #[must_use]
    pub const fn new(condition: IRExprID, true_block: BlockID, false_block: BlockID) -> Self {
        Self { condition, true_block, false_block }
    }

    #[must_use]
    pub const fn condition(&self) -> IRExprID { self.condition }

    #[must_use]
    pub const fn then_block(&self) -> BlockID { self.true_block }

    #[must_use]
    pub const fn else_block(&self) -> BlockID { self.false_block }
}

/// Ends a block at a suspension point: an expression from which control may
/// not come back, because the function may be suspended there.
///
/// The expression is still an expression and still defines its value. The
/// terminator only says where it is evaluated: at the point of the
/// terminator, with the value defined on the resume edge only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Suspend {
    expression: IRExprID,
    pub(super) resume: BlockID,
    pub(super) abandon: Option<BlockID>,
}

impl Suspend {
    #[must_use]
    pub const fn new(expression: IRExprID, resume: BlockID, abandon: Option<BlockID>) -> Self {
        Self { expression, resume, abandon }
    }

    /// Returns the `Call`, `Perform` or `RunWith` expression evaluated here.
    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    /// Returns the block that continues with the value of the expression.
    #[must_use]
    pub const fn resume_block(&self) -> BlockID { self.resume }

    /// Returns the block taken when the function is dropped while it is
    /// suspended here, if there is one.
    #[must_use]
    pub const fn abandon_block(&self) -> Option<BlockID> { self.abandon }
}

/// One operation clause of a [`Handle`]: the block that runs when the
/// coroutine performs the operation, and the values the coroutine suspended
/// with. The [`Handle`] keys it by the operation.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct HandleClause {
    yielded: Vec<IRExprID>,
    pub(super) block: BlockID,
}

impl HandleClause {
    #[must_use]
    pub const fn new(yielded: Vec<IRExprID>, block: BlockID) -> Self { Self { yielded, block } }

    /// Returns the `Yielded` expressions the clause receives: the arguments
    /// of the operation, followed by the continuation of the rest of the
    /// coroutine.
    ///
    /// The terminator defines them, on the edge into the clause only. No
    /// instruction evaluates them.
    #[must_use]
    pub fn yielded(&self) -> &[IRExprID] { &self.yielded }

    /// Returns the block of the clause.
    #[must_use]
    pub const fn block(&self) -> BlockID { self.block }
}

/// Ends a block by resuming a coroutine, and continues in exactly one of its
/// successors: the return block when the coroutine completes, or the block of
/// the clause for the operation it performs.
///
/// The clauses are branches of the function, like the arms of a conditional.
/// None is a nested function, and there are no captures.
///
/// An operation without a clause is forwarded, and the coroutine is resumed
/// with its result. That polling loop is not written out: one `Handle` stands
/// for all of it.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Handle {
    expression: IRExprID,
    pub(super) return_block: BlockID,
    clauses: FxHashMap<GlobalSymbolID, HandleClause>,
    pub(super) abandon: Option<BlockID>,
}

impl Handle {
    #[must_use]
    pub const fn new(
        expression: IRExprID,
        return_block: BlockID,
        clauses: FxHashMap<GlobalSymbolID, HandleClause>,
        abandon: Option<BlockID>,
    ) -> Self {
        Self { expression, return_block, clauses, abandon }
    }

    /// Returns the `Handle` expression evaluated here, which holds the
    /// continuation and the value it is resumed with.
    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    /// Returns the block taken when the coroutine completes. The value it
    /// returned is the value of the expression, defined on this edge only.
    #[must_use]
    pub const fn return_block(&self) -> BlockID { self.return_block }

    /// Iterates over the operation clauses with the operation each runs for,
    /// in no particular order.
    #[must_use]
    pub fn clauses(&self) -> impl ExactSizeIterator<Item = (GlobalSymbolID, &HandleClause)> {
        self.clauses.iter().map(|(operation_id, clause)| (*operation_id, clause))
    }

    /// Returns the block of the clause for `operation_id`, which the
    /// [`ControlFlowEdgeKind::HandleClause`] edge of that operation targets.
    pub(super) fn clause_block_mut(&mut self, operation_id: GlobalSymbolID) -> &mut BlockID {
        &mut self.clauses.get_mut(&operation_id).expect("Clause edge should have a clause").block
    }

    /// Returns the block taken when the function is dropped while it waits
    /// for a forwarded operation, if there is one.
    #[must_use]
    pub const fn abandon_block(&self) -> Option<BlockID> { self.abandon }
}

/// Describes the kind of control-flow edge between two blocks in a control-flow
/// graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum ControlFlowEdgeKind {
    /// A control-flow edge that is always taken.
    Jump,
    /// A control-flow edge that is taken when a condition evaluates to true.
    ConditionalTrue,
    /// A control-flow edge that is taken when a condition evaluates to false.
    ConditionalFalse,
    /// A control-flow edge that is taken when a suspension point continues
    /// with its value.
    SuspendResume,
    /// A control-flow edge that is taken when the function is dropped while
    /// it is suspended.
    SuspendAbandon,
    /// A control-flow edge that is taken when a resumed coroutine completes.
    HandleReturn,
    /// A control-flow edge that is taken when a resumed coroutine performs
    /// this operation, which has a clause.
    HandleClause(GlobalSymbolID),
    /// A control-flow edge that is taken when the function is dropped while
    /// a resumed coroutine waits for an operation it forwarded.
    HandleAbandon,
}

/// Describes a control-flow edge between two blocks in a control-flow graph.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct ControlFlowEdge {
    pub(super) kind: ControlFlowEdgeKind,
    pub(super) source: BlockID,
    pub(super) target: BlockID,
}

impl ControlFlowEdge {
    #[must_use]
    pub const fn kind(&self) -> ControlFlowEdgeKind { self.kind }

    #[must_use]
    pub const fn source(&self) -> BlockID { self.source }

    #[must_use]
    pub const fn target(&self) -> BlockID { self.target }
}

/// The single operation that transfers control out of a sealed block.
///
/// `Return(None)` is the canonical form of a bare or implicit unit return.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum Terminator {
    Jump(BlockID),
    Conditional(Conditional),
    Return(Option<IRExprID>),
    /// Evaluates an expression at a suspension point. See [`Suspend`].
    Suspend(Suspend),
    /// Resumes a coroutine and branches on how it stops. See [`Handle`].
    Handle(Handle),
}

impl Terminator {
    pub fn jump_targets(&self) -> impl Iterator<Item = BlockID> + '_ {
        // Only a `Handle` has more than two successors: its clauses, between
        // the return block and the abandon block.
        let (first, clauses, last) = match self {
            Self::Jump(block_id) => (Some(*block_id), None, None),
            Self::Conditional(conditional) => {
                (Some(conditional.true_block), None, Some(conditional.false_block))
            }
            Self::Return(_) => (None, None, None),
            Self::Suspend(suspend) => (Some(suspend.resume), None, suspend.abandon),
            Self::Handle(handle) => {
                (Some(handle.return_block), Some(&handle.clauses), handle.abandon)
            }
        };

        first
            .into_iter()
            .chain(
                clauses.into_iter().flat_map(|clauses| clauses.values().map(|clause| clause.block)),
            )
            .chain(last)
    }

    /// Returns whether this terminator returns from the function.
    #[must_use]
    pub const fn is_return(&self) -> bool {
        match self {
            Self::Return(_) => true,
            Self::Jump(_) | Self::Conditional(_) | Self::Suspend(_) | Self::Handle(_) => false,
        }
    }

    /// Returns the value this terminator returns from the function, if any.
    #[must_use]
    pub const fn returned_value(&self) -> Option<IRExprID> {
        match self {
            Self::Return(value) => *value,
            Self::Jump(_) | Self::Conditional(_) | Self::Suspend(_) | Self::Handle(_) => None,
        }
    }

    /// Returns the value this terminator consumes, if any: the condition of
    /// a branch, or the returned value.
    ///
    /// The operands of an [evaluated expression](Self::evaluated_expressions)
    /// are consumed by that expression, not by the terminator.
    #[must_use]
    pub const fn used_value(&self) -> Option<IRExprID> {
        match self {
            Self::Conditional(conditional) => Some(conditional.condition()),
            Self::Return(value) => *value,
            Self::Jump(_) | Self::Suspend(_) | Self::Handle(_) => None,
        }
    }

    /// Returns the expressions this terminator evaluates, which no
    /// instruction does. Each is defined on one of its outgoing edges only:
    ///
    /// - the expression of a suspension point, on the resume edge;
    /// - the `Handle` expression of a resumed coroutine, on the return edge,
    ///   followed by the `Yielded` expressions of each clause, on the edge into
    ///   that clause.
    ///
    /// The expression that the terminator is written for comes first.
    pub fn evaluated_expressions(&self) -> impl Iterator<Item = IRExprID> + '_ {
        let (expression, clauses) = match self {
            Self::Suspend(suspend) => (Some(suspend.expression), None),
            Self::Handle(handle) => (Some(handle.expression), Some(&handle.clauses)),
            Self::Jump(_) | Self::Conditional(_) | Self::Return(_) => (None, None),
        };

        expression.into_iter().chain(
            clauses
                .into_iter()
                .flat_map(FxHashMap::values)
                .flat_map(|clause| clause.yielded.iter().copied()),
        )
    }
}
