use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::BlockID,
    ir_expr::{IRExprID, call::Call},
    suspension_point::SuspendingExpr,
};
use rayc_solver::Solver;
use rayc_type::ty::{Ty, application::View as ApplicationView};

use crate::{layout_context::LayoutContext, savable::Savable};

/// Used to identify a suspension point in the IR. Typically, it's the terminal
/// of a particular block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct SuspensionPointID(BlockID);

impl SuspensionPointID {
    /// Identifies the suspension point that terminates `block_id`.
    ///
    /// Nothing checks that the block ends in a suspension; an ID of a block
    /// that does not is simply never found among the suspension points.
    #[must_use]
    pub const fn new(block_id: BlockID) -> Self { Self(block_id) }

    #[must_use]
    pub const fn block_id(&self) -> BlockID { self.0 }
}

/// What the function is suspended on at a suspension point, which tells what
/// it keeps there besides the values live across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuspendedOn {
    /// A handler outside the function, which a `perform` waits for. The
    /// function keeps nothing for it.
    Handler,

    /// A coroutine that the function owns, kept in the child frame of the
    /// suspension point: the frame of the callee of the call `expression`, or
    /// the frames of the body and the clauses of the `run ... with`
    /// `expression`.
    ChildFrame(IRExprID),

    /// A continuation: the operand of a `handle` or of a call of one. It only
    /// points to a frame that something else owns, so the function keeps the
    /// operand itself across the suspension point, as an ordinary value,
    /// rather than a child frame.
    Continuation(IRExprID),
}

impl SuspendedOn {
    /// The expression whose coroutine the child frame holds, if there is one.
    pub(crate) const fn child_frame(self) -> Option<IRExprID> {
        match self {
            Self::ChildFrame(expression) => Some(expression),
            Self::Handler | Self::Continuation(_) => None,
        }
    }

    /// The value held across the suspension point for what the function is
    /// suspended on, if any: the operand that holds the continuation.
    pub(crate) const fn held_value(self) -> Option<Savable> {
        match self {
            Self::Continuation(operand) => Some(Savable::Expression(operand)),
            Self::Handler | Self::ChildFrame(_) => None,
        }
    }
}

impl LayoutContext<'_> {
    /// Determines whether a suspending expression still counts as a suspension
    /// point after the substitution is applied to it. This is typically due to
    /// when an expression emits an opaque effect variable that is later
    /// substituted to an empty effect row, which means that the expression is
    /// no longer a suspension point.
    pub(crate) async fn is_still_a_suspension_point(
        &self,
        suspending: SuspendingExpr<'_>,
        solver: &mut Solver,
    ) -> bool {
        let effect_row = match suspending {
            SuspendingExpr::Perform => return true,
            SuspendingExpr::Call(call) => call.callee_effect_row(self.engine()).await,
            SuspendingExpr::RunWith(run_with) => run_with.residual_effect().clone(),
            SuspendingExpr::Handle(handle) => handle.forwarded_effect().clone(),
        };

        !self.latest_type(&effect_row, solver).await.is_empty_effect_row()
    }

    /// Iterates over every reachable suspension point of the IR function, with
    /// the suspending expression its terminator evaluates, whether or not it
    /// still suspends once the substitution is applied.
    pub(crate) fn ir_suspension_points(
        &self,
    ) -> impl Iterator<Item = (SuspensionPointID, IRExprID)> + '_ {
        self.ir_function().traverse().filter_map(|(block_id, block)| {
            let suspending_expr_id = block.terminator()?.suspending_expr()?;
            Some((SuspensionPointID(block_id), suspending_expr_id))
        })
    }

    /// Calculates what the function is suspended on at each suspension point
    /// where it still suspends once the substitution is applied.
    pub(crate) async fn calculate_suspended_on_for_each_suspension_point(
        &self,
        solver: &mut Solver,
    ) -> FxHashMap<SuspensionPointID, SuspendedOn> {
        let mut suspensions = FxHashMap::default();

        for (suspension_point_id, expression) in self.ir_suspension_points() {
            let suspending = self.ir_function().get_suspending_expr(expression);
            let is_suspension_point = self.is_still_a_suspension_point(suspending, solver).await;

            // is still considered as a suspension point after the substitution
            // is applied
            if is_suspension_point {
                let suspended_on = self.suspended_on(expression, suspending, solver).await;
                suspensions.insert(suspension_point_id, suspended_on);
            }
        }

        suspensions
    }

    /// What the function is suspended on while it is suspended at
    /// `suspending`, the expression `expression`.
    async fn suspended_on(
        &self,
        expression: IRExprID,
        suspending: SuspendingExpr<'_>,
        solver: &mut Solver,
    ) -> SuspendedOn {
        match suspending {
            SuspendingExpr::Perform => SuspendedOn::Handler,
            SuspendingExpr::Handle(handle) => SuspendedOn::Continuation(handle.continuation()),

            // `Def.call(k, arguments)` of a continuation takes it first. Any
            // other call runs its callee in a frame of its own.
            SuspendingExpr::Call(call) => {
                if self.calls_continuation(call, solver).await {
                    SuspendedOn::Continuation(call.arguments()[0])
                } else {
                    SuspendedOn::ChildFrame(expression)
                }
            }

            SuspendingExpr::RunWith(_) => SuspendedOn::ChildFrame(expression),
        }
    }

    /// Whether `call` dispatches through the `Def` dictionary of a
    /// continuation, once the substitution is applied.
    async fn calls_continuation(&self, call: &Call, solver: &mut Solver) -> bool {
        let Some(dictionary) = call.target().dispatch_instance() else {
            return false;
        };

        matches!(
            &*self.latest_type(dictionary, solver).await,
            Ty::Application(application)
                if matches!(application.view(), ApplicationView::ContinuationDefInstance(_))
        )
    }
}
