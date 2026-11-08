use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::BlockID,
    ir_expr::{IRExprID, call::Call},
    suspension_point::Waitee,
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

/// What the function keeps, besides the values live across a suspension
/// point, for what the expression it evaluates there waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Awaited {
    /// Nothing: a `perform` waits for a handler outside the function.
    Nothing,

    /// A coroutine that the function owns, in the awaitee of the suspension
    /// point: the frame of the callee of the call `expression`, or the frames
    /// of the body and the clauses of the `run ... with` `expression`.
    Awaitee(IRExprID),

    /// A continuation: the operand of a `handle` or of a call of one. It only
    /// points to a frame that something else owns, so the function holds the
    /// operand itself across the suspension point, as an ordinary value,
    /// rather than an awaitee.
    Continuation(IRExprID),
}

impl Awaited {
    /// The expression whose coroutine the awaitee holds, if there is one.
    pub(crate) const fn awaitee(self) -> Option<IRExprID> {
        match self {
            Self::Awaitee(expression) => Some(expression),
            Self::Nothing | Self::Continuation(_) => None,
        }
    }

    /// The value held across the suspension point for what it waits for, if
    /// any: the operand that holds the continuation.
    pub(crate) const fn held_value(self) -> Option<Savable> {
        match self {
            Self::Continuation(operand) => Some(Savable::Expression(operand)),
            Self::Nothing | Self::Awaitee(_) => None,
        }
    }
}

impl LayoutContext<'_> {
    /// Determines whether a waitee still counts as a suspension point after the
    /// substitution is applied to it. This is typically due to when an
    /// expression emits an opaque effect variable that is later substituted
    /// to an empty effect row, which means that the expression is no longer
    /// a suspension point.
    pub(crate) async fn is_waitee_still_a_suspension_point(
        &self,
        waitee: Waitee<'_>,
        solver: &mut Solver,
    ) -> bool {
        let effect_row = match waitee {
            Waitee::Perform => return true,
            Waitee::Call(call) => call.callee_effect_row(self.engine()).await,
            Waitee::RunWith(run_with) => run_with.residual_effect().clone(),
            Waitee::Handle(handle) => handle.forwarded_effect().clone(),
        };

        !self.latest_type(&effect_row, solver).await.is_empty_effect_row()
    }

    /// Iterates over every reachable suspension point of the IR function, with
    /// the waitee expression its terminator evaluates, whether or not it still
    /// suspends once the substitution is applied.
    pub(crate) fn ir_suspension_points(
        &self,
    ) -> impl Iterator<Item = (SuspensionPointID, IRExprID)> + '_ {
        self.ir_function().traverse().filter_map(|(block_id, block)| {
            let suspending_expr_id = block.terminator()?.suspending_expr()?;
            Some((SuspensionPointID(block_id), suspending_expr_id))
        })
    }

    /// Calculates what the function keeps for each suspension point where it
    /// still suspends once the substitution is applied.
    pub(crate) async fn calculate_awaited_for_each_suspension_point(
        &self,
        solver: &mut Solver,
    ) -> FxHashMap<SuspensionPointID, Awaited> {
        let mut suspensions = FxHashMap::default();

        for (suspension_point_id, waitee_expr_id) in self.ir_suspension_points() {
            let waitee = self.ir_function().get_expression_as_waitee(waitee_expr_id);
            let is_suspension_point = self.is_waitee_still_a_suspension_point(waitee, solver).await;

            // is still considered as a suspension point after the substitution
            // is applied
            if is_suspension_point {
                let awaited = self.awaited(waitee_expr_id, waitee, solver).await;
                suspensions.insert(suspension_point_id, awaited);
            }
        }

        suspensions
    }

    /// What the function keeps for `waitee`, the expression `expression`,
    /// while it is suspended at it.
    async fn awaited(
        &self,
        expression: IRExprID,
        waitee: Waitee<'_>,
        solver: &mut Solver,
    ) -> Awaited {
        match waitee {
            Waitee::Perform => Awaited::Nothing,
            Waitee::Handle(handle) => Awaited::Continuation(handle.continuation()),

            // `Def.call(k, arguments)` of a continuation takes it first. Any
            // other call runs its callee in a frame of its own.
            Waitee::Call(call) => {
                if self.calls_continuation(call, solver).await {
                    Awaited::Continuation(call.arguments()[0])
                } else {
                    Awaited::Awaitee(expression)
                }
            }

            Waitee::RunWith(_) => Awaited::Awaitee(expression),
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
