use rayc_ir::{
    cfg::{BlockID as IRBlockID, Instruction as IRInstruction, Terminator as IRTerminator},
    ir_expr::IRExprKind,
};
use rayc_mono_ir::{
    cfg::{BlockID, Branch, Terminator},
    operand::{Constant, Operand},
};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    pub(super) async fn lower_terminator(
        &mut self,
        source_block: IRBlockID,
        terminator: &IRTerminator,
    ) {
        let terminator = match terminator {
            // In direct form a suspension point evaluates its expression and
            // continues with the value. Its abandon edge is never taken. A
            // coroutine only does so where the substitution emptied the row.
            IRTerminator::Suspend(suspend) => {
                if let Some(variant) = self.suspension_variant(source_block) {
                    self.lower_yield(source_block, suspend, variant);
                    return;
                }
                self.lower_expression(suspend.expression()).await;
                Terminator::Goto(self.lower_edge(source_block, suspend.resume_block()))
            }
            IRTerminator::Handle(handle) => {
                self.lower_handle(source_block, handle).await;
                return;
            }
            IRTerminator::Jump(successor) => {
                Terminator::Goto(self.lower_edge(source_block, *successor))
            }
            IRTerminator::Conditional(conditional) => Terminator::Branch(Branch::new(
                self.expression_operand(conditional.condition()),
                self.lower_edge(source_block, conditional.then_block()),
                self.lower_edge(source_block, conditional.else_block()),
            )),
            // Only an abandon edge reaches it, and direct form never takes
            // one, so abandon blocks are not lowered.
            IRTerminator::Abandon => {
                unreachable!("an abandon block is not lowered in direct form")
            }
            IRTerminator::Return(value) => {
                let value = value.map_or_else(
                    || Operand::Constant(Constant::Unit),
                    |value| self.expression_operand(value),
                );

                if self.is_coroutine() {
                    self.lower_coroutine_return(value);
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
    pub(super) fn lower_edge(&mut self, predecessor: IRBlockID, successor: IRBlockID) -> BlockID {
        let target = self.block(successor);

        // place all incoming phi values into their destination locals.
        let copies = self
            .source_block_instructions(successor)
            .iter()
            .filter_map(|instruction| {
                let IRInstruction::Expression(expression_id) = instruction else {
                    return None;
                };
                let IRExprKind::Phi(phi) = self.source_expression(*expression_id).kind() else {
                    return None;
                };
                let incoming = phi.value_from(predecessor).unwrap_or_else(|| {
                    panic!("phi {expression_id:?} has no input for predecessor {predecessor:?}")
                });
                Some((self.expression_place(*expression_id), self.expression_operand(incoming)))
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
