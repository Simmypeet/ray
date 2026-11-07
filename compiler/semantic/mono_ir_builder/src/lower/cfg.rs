use rayc_ir::{
    cfg::{BlockID as IRBlockID, Instruction as IRInstruction, Terminator as IRTerminator},
    ir_expr::IRExprKind,
};
use rayc_mono_ir::{
    cfg::{BlockID, Branch, Terminator},
    operand::{Constant, Operand},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    pub(super) async fn lower_terminator(
        &mut self,
        source_block: IRBlockID,
        terminator: &IRTerminator,
        ctx: &Context<'_>,
    ) {
        let terminator = match terminator {
            // In direct form a suspension point evaluates its expression and
            // continues with the value. Its abandon edge is never taken. A
            // coroutine only does so where the substitution emptied the row.
            IRTerminator::Suspend(suspend) => {
                if let Some(point) = ctx.suspension_point(source_block) {
                    self.lower_suspension(point, suspend, ctx).await;
                    return;
                }
                self.lower_expression(suspend.expression(), ctx).await;
                Terminator::Goto(self.lower_edge(source_block, suspend.resume_block(), ctx))
            }
            IRTerminator::Handle(handle) => {
                self.lower_handle(source_block, handle, ctx).await;
                return;
            }
            IRTerminator::Jump(successor) => {
                Terminator::Goto(self.lower_edge(source_block, *successor, ctx))
            }
            IRTerminator::Conditional(conditional) => Terminator::Branch(Branch::new(
                ctx.expression_operand(conditional.condition()),
                self.lower_edge(source_block, conditional.then_block(), ctx),
                self.lower_edge(source_block, conditional.else_block(), ctx),
            )),
            // Only an abandon edge reaches it, and direct form never takes
            // one, so abandon blocks are not lowered.
            IRTerminator::Abandon => {
                unreachable!("an abandon block is not lowered in direct form")
            }
            IRTerminator::Return(value) => {
                let value = value.map_or_else(
                    || Operand::Constant(Constant::Unit),
                    |value| ctx.expression_operand(value),
                );

                if ctx.is_coroutine() {
                    self.lower_complete_coroutine(value, ctx);
                    return;
                }
                Terminator::Return(Some(value))
            }
        };
        self.set_terminator(terminator);
    }

    /// Splits an incoming edge when its successor contains phi expressions.
    ///
    /// All incoming values are first copied to temporaries, then written to
    /// their phi destinations. This preserves parallel-copy semantics when phi
    /// inputs and destinations overlap.
    pub(super) fn lower_edge(
        &mut self,
        predecessor: IRBlockID,
        successor: IRBlockID,
        ctx: &Context<'_>,
    ) -> BlockID {
        let target = ctx.block(successor);

        // place all incoming phi values into their destination locals.
        let copies = ctx
            .source_block_instructions(successor)
            .iter()
            .filter_map(|instruction| {
                let IRInstruction::Expression(expression_id) = instruction else {
                    return None;
                };
                let IRExprKind::Phi(phi) = ctx.source_expression(*expression_id).kind() else {
                    return None;
                };
                let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                    panic!("phi {expression_id:?} has no input for predecessor {predecessor:?}")
                });
                Some((ctx.expression_place(*expression_id), ctx.expression_operand(incoming)))
            })
            .collect::<Vec<_>>();

        if copies.is_empty() {
            return target;
        }

        // create a temporary block between the predecessor and successor to
        // perform the phi copies
        self.in_new_block(|emit| {
            for (destination, value) in copies {
                emit.store(destination, value);
            }

            emit.set_terminator(Terminator::Goto(target));
        })
    }
}
