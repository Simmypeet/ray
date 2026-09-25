use rayc_ir::ir_expr::{IRExpr, IRExprKind, tuple::Tuple};
use rayc_typed_ast::typed_expr::statement_block::StatementBlock;

use crate::{
    builder::{Builder, function_build_state::scope_tracker::ScopeKind},
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a StatementBlock>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a StatementBlock>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());

        // The block is a lexical scope, so its variables die at its end.
        self.enter_scope(ScopeKind::Lexical);
        self.lower_statement_list(context, expression.node().statements()).await;
        self.exit_scope();

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::Tuple(Tuple::new(Vec::new())),
            typed_expression.span(),
            typed_expression.ty().clone(),
        )))
    }
}
