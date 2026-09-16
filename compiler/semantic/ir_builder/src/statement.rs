use rayc_ir::cfg::{BlockID, Terminator};
use rayc_typed_ast::statement::Statement;

use crate::{builder::Builder, context::LoweringContext};

#[derive(Clone, Copy)]
pub(crate) struct LoopTarget {
    break_target: BlockID,
    continue_target: BlockID,
}

impl LoopTarget {
    pub(crate) const fn new(break_target: BlockID, continue_target: BlockID) -> Self {
        Self { break_target, continue_target }
    }
}

impl Builder {
    pub fn lower_statements(&mut self, context: &LoweringContext<'_>) {
        self.lower_statement_list(context, context.statements());
    }

    pub(crate) fn lower_statement_list<'a>(
        &mut self,
        context: &LoweringContext<'_>,
        statements: impl IntoIterator<Item = &'a Statement>,
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
                        let value = self.lower_rvalue_by_id(context, expr_id);
                        self.emit_store(self.variable_address(ir_id), value);
                    }
                }
                Statement::Break(_) => {
                    if let Some(loop_target) = self.current_loop_target() {
                        self.jump_to(loop_target.break_target);
                    }
                }
                Statement::Continue(_) => {
                    if let Some(loop_target) = self.current_loop_target() {
                        self.jump_to(loop_target.continue_target);
                    }
                }
                Statement::Expression(expression) => {
                    self.lower_rvalue_by_id(context, *expression);
                }
                Statement::Return(return_statement) => {
                    let value = return_statement
                        .value()
                        .map(|value| self.lower_rvalue_by_id(context, value));
                    self.terminate(Terminator::Return(value));
                }
            }
        }
    }
}
