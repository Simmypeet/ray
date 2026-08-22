use rayc_ir::cfg::Terminator;
use rayc_typed_ast::{function::Function as TypedFunction, statement::Statement};

use crate::builder::Builder;

impl Builder {
    pub(crate) fn lower_statements(&mut self, typed_function: &TypedFunction) {
        for statement in typed_function.statements() {
            if self.is_terminated() {
                break;
            }

            match *statement {
                Statement::Let(let_statement) => {
                    let typed_id = let_statement.variable_id();
                    let ir_id = self.register_source_variable(typed_function, typed_id);
                    let value =
                        self.lower_expression_by_id(typed_function, let_statement.expression());
                    self.emit_store(self.variable_address(ir_id), value);
                }
                Statement::Expression(expression) => {
                    self.lower_expression_by_id(typed_function, expression);
                }
                Statement::Return(return_statement) => {
                    let value = return_statement
                        .value()
                        .map(|value| self.lower_expression_by_id(typed_function, value));
                    self.terminate(Terminator::Return(value));
                }
            }
        }

        if !self.is_terminated() {
            self.terminate(Terminator::Return(None));
        }
    }
}
