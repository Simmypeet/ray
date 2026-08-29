use rayc_ir::cfg::Terminator;
use rayc_typed_ast::statement::Statement;

use crate::{builder::Builder, context::LoweringContext};

impl Builder {
    pub fn lower_statements(&mut self, context: &LoweringContext<'_>) {
        for statement in context.statements() {
            if self.is_terminated() {
                break;
            }

            match *statement {
                Statement::Let(let_statement) => {
                    let typed_id = let_statement.variable_id();
                    let ir_id = self.register_source_variable(context, typed_id);

                    if let Some(expr_id) = let_statement.expression() {
                        let value = self.lower_expression_by_id(context, expr_id);
                        self.emit_store(self.variable_address(ir_id), value);
                    }
                }
                Statement::Expression(expression) => {
                    self.lower_expression_by_id(context, expression);
                }
                Statement::Return(return_statement) => {
                    let value = return_statement
                        .value()
                        .map(|value| self.lower_expression_by_id(context, value));
                    self.terminate(Terminator::Return(value));
                }
            }
        }

        if !self.is_terminated() {
            self.terminate(Terminator::Return(None));
        }
    }
}
