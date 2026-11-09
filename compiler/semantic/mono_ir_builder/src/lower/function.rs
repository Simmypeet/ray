use rayc_ir::cfg::Instruction as IRInstruction;

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers the function in callback style.
    pub(crate) async fn lower_function(mut self, ctx: &Context<'_>) {
        self.lower_blocks(ctx).await;
    }

    /// Lowers every block that direct control flow reaches.
    pub(super) async fn lower_blocks(&mut self, ctx: &Context<'_>) {
        for source_block in ctx.source_direct_reachables().blocks() {
            self.select_block(ctx.block(source_block));
            for instruction in ctx.source_block_instructions(source_block) {
                self.lower_instruction(instruction, ctx).await;
            }

            let terminator = ctx
                .source_block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            self.lower_terminator(source_block, terminator, ctx).await;
        }
    }

    pub(super) async fn lower_instruction(
        &mut self,
        instruction: &IRInstruction,
        ctx: &Context<'_>,
    ) {
        match instruction {
            IRInstruction::ScopePush(_) | IRInstruction::ScopePop(_) => {}
            IRInstruction::ExprDiscard(discard) => self.lower_expr_discard(discard, ctx).await,
            IRInstruction::AddressDrop(drop) => self.lower_address_drop(drop, ctx).await,
            IRInstruction::Expression(expression_id) => {
                self.lower_expression(*expression_id, ctx).await;
            }
            IRInstruction::Store(store) => {
                let destination = self.lower_address(store.address(), ctx);
                self.store(destination, ctx.expression_operand(store.expression()));
            }
        }
    }
}
