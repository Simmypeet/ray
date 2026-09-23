use rayc_ir::cfg::{BlockID, Terminator};
use rayc_typed_ast::statement::Statement;

use crate::{
    builder::{Builder, function_build_state::scope_tracker::ScopeKind},
    context::LoweringContext,
};

#[derive(Clone, Copy)]
pub(crate) struct LoopTarget {
    break_target: BlockID,
    continue_target: BlockID,
    scope_depth: usize,
}

impl LoopTarget {
    pub(crate) const fn new(
        break_target: BlockID,
        continue_target: BlockID,
        scope_depth: usize,
    ) -> Self {
        Self { break_target, continue_target, scope_depth }
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

            // Temporaries produced while lowering one statement live only for that
            // statement.
            self.enter_scope(ScopeKind::Temporary);
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
                        self.unwind_scopes_from(loop_target.scope_depth);
                        self.jump_to(loop_target.break_target);
                    }
                }
                Statement::Continue(_) => {
                    if let Some(loop_target) = self.current_loop_target() {
                        self.unwind_scopes_from(loop_target.scope_depth);
                        self.jump_to(loop_target.continue_target);
                    }
                }
                Statement::Expression(expression) => {
                    let value = self.lower_rvalue_by_id(context, *expression);
                    self.emit_expr_discard(value);
                }
                Statement::Return(return_statement) => {
                    let value = return_statement
                        .value()
                        .map(|value| self.lower_rvalue_by_id(context, value));
                    self.unwind_all_scopes();
                    self.terminate(Terminator::Return(value));
                }
            }
            self.exit_scope();
        }
    }
}
