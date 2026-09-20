use rayc_ir::{
    cfg::{Conditional, Terminator},
    ir_expr::{IRExpr, IRExprKind, tuple::Tuple},
};
use rayc_typed_ast::typed_expr::while_loop::While;

use crate::{
    builder::{Builder, function_build_state::scope_tracker::ScopeKind},
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
    statement::LoopTarget,
};

impl<'a> Lower<TypedExprWithID<&'a While>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a While>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let while_loop = expression.node();

        // Re-enter the condition block after every normal body fallthrough or
        // continue; break branches directly to the exit block.
        let condition_block = self.create_block();
        let body_block = self.create_block();
        let exit_block = self.create_block();
        self.jump_to(condition_block);

        self.select_block(condition_block);
        let condition = self.lower_rvalue_by_id(context, while_loop.condition());
        self.terminate(Terminator::Conditional(Conditional::new(
            condition, body_block, exit_block,
        )));

        self.select_block(body_block);
        let loop_scope_depth = self.scope_depth();
        self.enter_scope(ScopeKind::Lexical);
        self.push_loop_target(LoopTarget::new(exit_block, condition_block, loop_scope_depth));
        self.lower_statement_list(context, while_loop.body());
        self.pop_loop_target();
        self.exit_scope();
        if !self.is_terminated() {
            self.jump_to(condition_block);
        }

        self.select_block(exit_block);
        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Tuple(Tuple::new(Vec::new())),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
