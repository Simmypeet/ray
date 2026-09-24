use rayc_ir::{
    cfg::{Conditional, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, phi::Phi},
};
use rayc_typed_ast::typed_expr::if_else::{Arm, IfElse};

use crate::{
    builder::{Builder, function_build_state::scope_tracker::ScopeKind},
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a IfElse>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a IfElse>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let if_else = expression.node();

        // Lower the conditional arms as a chain whose false edges advance to
        // the next condition (or the final else path).
        let merge_block = self.create_block();
        let mut incoming = Vec::new();
        let branch_count =
            if_else.conditional_arms().count() + usize::from(if_else.else_arm().is_some());
        let mut arm_scopes = self.create_scope_branch(branch_count).into_iter();

        for conditional_arm in if_else.conditional_arms() {
            let condition = self.lower_rvalue_by_id(context, conditional_arm.condition()).await;
            let arm_block = self.create_block();
            let next_condition_block = self.create_block();
            self.terminate(Terminator::Conditional(Conditional::new(
                condition,
                arm_block,
                next_condition_block,
            )));

            self.select_block(arm_block);
            self.enter_existing_scope(
                arm_scopes.next().expect("a conditional arm scope should exist"),
                ScopeKind::Lexical,
            );
            let value = self.lower_arm(context, conditional_arm.arm(), span, &ty).await;
            self.exit_scope();
            if let Some(value) = value {
                incoming.push((self.jump_to(merge_block), value));
            }

            self.select_block(next_condition_block);
        }

        // A missing else is the implicit unit-valued fallthrough path.
        let else_value = if let Some(else_arm) = if_else.else_arm() {
            self.enter_existing_scope(
                arm_scopes.next().expect("an else arm scope should exist"),
                ScopeKind::Lexical,
            );
            let value = self.lower_arm(context, else_arm, span, &ty).await;
            self.exit_scope();
            value
        } else {
            Some(self.emit_unit(span, ty.clone()))
        };
        assert!(arm_scopes.next().is_none(), "all if arm scopes should be lowered");
        if let Some(value) = else_value {
            incoming.push((self.jump_to(merge_block), value));
        }

        self.select_block(merge_block);
        if incoming.is_empty() {
            LoweredExpression::RValue(self.emit_unit(span, ty))
        } else {
            LoweredExpression::RValue(self.emit_expression(IRExpr::new(
                IRExprKind::Phi(Phi::new(incoming.into_iter().collect())),
                span,
                ty,
            )))
        }
    }
}

impl Builder {
    async fn lower_arm(
        &mut self,
        context: &LoweringContext<'_>,
        arm: &Arm,
        span: rayc_lexical::tree::RelativeSpan,
        ty: &qbice::storage::intern::Interned<rayc_type::ty::Ty>,
    ) -> Option<IRExprID> {
        match arm {
            Arm::Expression(expression) => {
                Some(self.lower_rvalue_by_id(context, *expression).await)
            }
            Arm::Block(statements) => {
                self.lower_statement_list(context, statements).await;
                (!self.is_terminated()).then(|| self.emit_unit(span, ty.clone()))
            }
        }
    }
}
