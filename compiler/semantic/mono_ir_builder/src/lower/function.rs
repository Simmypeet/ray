use rayc_ir::cfg::Instruction as IRInstruction;

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    /// Lowers the function in callback style.
    pub(crate) async fn lower_function(mut self) { self.lower_blocks().await; }

    /// Lowers every block that direct control flow reaches.
    pub(super) async fn lower_blocks(&mut self) {
        for source_block in self.source_direct_reachables().blocks() {
            self.select_block(self.block(source_block));
            for instruction in self.source_block_instructions(source_block) {
                self.lower_instruction(instruction).await;
            }

            let terminator = self
                .source_block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            self.lower_terminator(source_block, terminator).await;
        }
    }

    async fn lower_instruction(&mut self, instruction: &IRInstruction) {
        match instruction {
            IRInstruction::ScopePush(_) | IRInstruction::ScopePop(_) => {}
            IRInstruction::ExprDiscard(discard) => self.lower_expr_discard(discard).await,
            IRInstruction::AddressDrop(drop) => self.lower_address_drop(drop).await,
            IRInstruction::Expression(expression_id) => {
                self.lower_expression(*expression_id).await;
            }
            IRInstruction::Store(store) => {
                let destination = self.lower_address(store.address());
                self.store(destination, self.expression_operand(store.expression()));
            }
        }
    }
}
