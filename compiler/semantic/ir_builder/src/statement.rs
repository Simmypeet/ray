use rayc_ir::cfg::{BlockID, Conditional, Terminator};
use rayc_typed_ast::statement::Statement;

use crate::{builder::Builder, context::LoweringContext};

#[derive(Clone, Copy)]
struct LoopTarget {
    break_target: BlockID,
    continue_target: BlockID,
}

impl LoopTarget {
    const fn new(break_target: BlockID, continue_target: BlockID) -> Self {
        Self { break_target, continue_target }
    }
}

impl Builder {
    pub fn lower_statements(&mut self, context: &LoweringContext<'_>) {
        self.lower_statement_list(context, context.statements(), None);
    }

    fn lower_statement_list<'a>(
        &mut self,
        context: &LoweringContext<'_>,
        statements: impl IntoIterator<Item = &'a Statement>,
        loop_target: Option<LoopTarget>,
    ) {
        for statement in statements {
            if self.is_terminated() {
                break;
            }

            match statement {
                Statement::Let(let_statement) => {
                    let typed_id = let_statement.variable_id();
                    let ir_id = self.register_source_variable(context, typed_id);

                    if let Some(expr_id) = let_statement.expression() {
                        let value = self.lower_expression_by_id(context, expr_id);
                        self.emit_store(self.variable_address(ir_id), value);
                    }
                }
                Statement::While(while_statement) => {
                    // statement Enter the loop through a dedicated condition
                    // block so back edges reevaluate the condition on every
                    // iteration.
                    let condition_block = self.create_block();
                    let body_block = self.create_block();
                    let exit_block = self.create_block();
                    self.jump_to(condition_block);

                    self.select_block(condition_block);
                    let condition =
                        self.lower_expression_by_id(context, while_statement.condition());
                    self.terminate(Terminator::Conditional(Conditional::new(
                        condition, body_block, exit_block,
                    )));

                    // A fallthrough or `continue` repeats the condition; `break` jumps
                    // directly to the block selected after lowering the body.
                    self.select_block(body_block);
                    self.lower_statement_list(
                        context,
                        while_statement.body(),
                        Some(LoopTarget::new(exit_block, condition_block)),
                    );
                    if !self.is_terminated() {
                        self.jump_to(condition_block);
                    }

                    self.select_block(exit_block);
                }
                Statement::Break(_) => {
                    if let Some(loop_target) = loop_target {
                        self.jump_to(loop_target.break_target);
                    }
                }
                Statement::Continue(_) => {
                    if let Some(loop_target) = loop_target {
                        self.jump_to(loop_target.continue_target);
                    }
                }
                Statement::Expression(expression) => {
                    self.lower_expression_by_id(context, *expression);
                }
                Statement::Return(return_statement) => {
                    let value = return_statement
                        .value()
                        .map(|value| self.lower_expression_by_id(context, value));
                    self.terminate(Terminator::Return(value));
                }
            }
        }
    }
}
