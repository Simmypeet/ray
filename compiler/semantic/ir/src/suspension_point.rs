//! What a function holds at each of its suspension points.
//!
//! A function suspends at every [`Terminator::Suspend`], and at every
//! [`Terminator::Handle`] that forwards an operation, which is one with an
//! abandon block. While it is suspended there, it holds:
//!
//! - the locals and expression values live at the exit of the terminator's
//!   block, on some edge out of it, use-live or drop-live, except the values
//!   the terminator itself defines, which do not exist yet;
//! - the locals whose storage may be borrowed there, which have to survive the
//!   suspension even when their values are dead;
//! - what the evaluated expression waits for, which the terminator consumes and
//!   so is not live after it.
//!
//! The captures of the function are never held: they live in its
//! environment, not in its frame. They are counted by whatever creates the
//! function.
//!
//! The arguments of a `perform` are not held either. They leave the function
//! with the operation, which is why the exit of the block is the point that
//! counts, not the terminator, which reads them.

use crate::{
    address::Local,
    borrowed_locals::MaybeBorrowedLocals,
    cfg::Point,
    ir_expr::{IRExprID, IRExprKind, call::Call, handle::Handle, run_with::RunWith},
    ir_function::IRFunction,
    liveness::{expr::ExprLiveness, local::LocalLiveness},
};

/// What the expression evaluated at a suspension point waits for while the
/// function is suspended there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waitee<'a> {
    /// A `perform`, which waits for the handler of the operation, outside the
    /// function. Its arguments leave with the operation.
    Perform,

    /// A call, which waits for the callee to complete. The arguments of the
    /// call are held by the callee.
    Call(&'a Call),

    /// A `run ... with`, which waits for its handled body and its operation
    /// handlers. Each is a nested function of the same function map, created
    /// from its capture operands.
    RunWith(&'a RunWith),

    /// A `handle` that forwards an operation, which waits for the
    /// continuation operand, consumed by the `handle`.
    Handle(&'a Handle),
}

/// One suspension point of a function, with what the function holds while it
/// is suspended there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspensionPoint<'a> {
    point: Point,
    expression: IRExprID,
    held_locals: Vec<Local>,
    held_expressions: Vec<IRExprID>,
    borrowed: Vec<Local>,
    waited: Waitee<'a>,
}

impl<'a> SuspensionPoint<'a> {
    /// Collects every reachable suspension point of `function`, in the order
    /// of their points.
    pub async fn collect(function: &'a IRFunction) -> Vec<Self> {
        let local_liveness = LocalLiveness::compute(function).await;
        let expr_liveness = ExprLiveness::compute(function).await;
        let borrowed_locals = MaybeBorrowedLocals::compute(function).await;

        let mut suspension_points = Vec::new();
        for (block_id, block) in function.traverse() {
            let Some(terminator) = block.terminator() else {
                continue;
            };

            let Some(expression) = terminator.suspension_waitee() else {
                continue;
            };

            let traversed = "traversed blocks are reachable";
            let held_locals = local_liveness
                .block_exit(block_id)
                .expect(traversed)
                .live()
                .filter(|local| !matches!(local, Local::Capture(_)))
                .collect();
            let held_expressions = expr_liveness
                .block_exit(block_id)
                .expect(traversed)
                .live()
                .filter(|value| {
                    !terminator.evaluated_expressions().any(|defined| defined == *value)
                })
                .collect();
            let borrowed = borrowed_locals.block_exit(block_id).expect(traversed).iter().collect();

            suspension_points.push(Self {
                point: function.terminator_point(block_id),
                expression,
                held_locals,
                held_expressions,
                borrowed,
                waited: Waitee::of(function.get_expression(expression).kind()),
            });
        }

        suspension_points.sort_unstable_by_key(|suspension_point| suspension_point.point);
        suspension_points
    }

    /// Returns the point of the terminator that suspends.
    #[must_use]
    pub const fn point(&self) -> Point { self.point }

    /// Returns the `Call`, `Perform`, `RunWith` or `Handle` expression the
    /// terminator evaluates.
    #[must_use]
    pub const fn expression(&self) -> IRExprID { self.expression }

    /// Returns the locals held across the suspension point, in order.
    #[must_use]
    pub fn held_locals(&self) -> &[Local] { &self.held_locals }

    /// Returns the expression values held across the suspension point, in
    /// order.
    #[must_use]
    pub fn held_expressions(&self) -> &[IRExprID] { &self.held_expressions }

    /// Returns the locals whose storage may be borrowed at the suspension
    /// point, in order. A local may also be held.
    #[must_use]
    pub fn borrowed(&self) -> &[Local] { &self.borrowed }

    /// Returns what the evaluated expression waits for.
    #[must_use]
    pub const fn waited(&self) -> Waitee<'a> { self.waited }
}

impl<'a> Waitee<'a> {
    /// Returns what the expression `kind`, evaluated at a suspension point,
    /// waits for.
    #[must_use]
    pub fn of(kind: &'a IRExprKind) -> Self {
        match kind {
            IRExprKind::Perform(_) => Self::Perform,
            IRExprKind::Call(call) => Self::Call(call),
            IRExprKind::RunWith(run_with) => Self::RunWith(run_with),
            IRExprKind::Handle(handle) => Self::Handle(handle),

            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::RefToPointer(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::Closure(_)
            | IRExprKind::Coroutine(_)
            | IRExprKind::Yielded(_)
            | IRExprKind::StructInitialization(_) => {
                unreachable!("only a call, a perform, a run or a handle suspends")
            }
        }
    }
}
