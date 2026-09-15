use rayc_ir::{
    cfg::{Conditional, Terminator},
    ir_expr::{IRExpr, IRExprID, IRExprKind, phi::Phi, tuple::Tuple},
};
use rayc_typed_ast::typed_expr::if_else::{Arm, IfElse};

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a IfElse>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a IfElse>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let if_else = expression.node();

        // Lower the conditional arms as a chain whose false edges advance to
        // the next condition (or the final else path).
        let merge_block = self.create_block();
        let mut incoming = Vec::new();

        for conditional_arm in if_else.conditional_arms() {
            let condition = self.lower_expression_by_id(context, conditional_arm.condition());
            let arm_block = self.create_block();
            let next_condition_block = self.create_block();
            self.terminate(Terminator::Conditional(Conditional::new(
                condition,
                arm_block,
                next_condition_block,
            )));

            self.select_block(arm_block);
            if let Some(value) = self.lower_arm(context, conditional_arm.arm(), span, &ty) {
                incoming.push((self.jump_to(merge_block), value));
            }

            self.select_block(next_condition_block);
        }

        // A missing else is the implicit unit-valued fallthrough path.
        let else_value = if let Some(else_arm) = if_else.else_arm() {
            self.lower_arm(context, else_arm, span, &ty)
        } else {
            Some(self.emit_unit(span, ty.clone()))
        };
        if let Some(value) = else_value {
            incoming.push((self.jump_to(merge_block), value));
        }

        self.select_block(merge_block);
        if incoming.is_empty() {
            self.emit_unit(span, ty)
        } else {
            self.emit_expression(IRExpr::new(
                IRExprKind::Phi(Phi::new(incoming.into_iter().collect())),
                span,
                ty,
            ))
        }
    }
}

impl Builder {
    fn lower_arm(
        &mut self,
        context: &LoweringContext<'_>,
        arm: &Arm,
        span: rayc_lexical::tree::RelativeSpan,
        ty: &qbice::storage::intern::Interned<rayc_type::ty::Ty>,
    ) -> Option<IRExprID> {
        match arm {
            Arm::Expression(expression) => Some(self.lower_expression_by_id(context, *expression)),
            Arm::Block(statements) => {
                self.lower_statement_list(context, statements);
                (!self.is_terminated()).then(|| self.emit_unit(span, ty.clone()))
            }
        }
    }

    fn emit_unit(
        &mut self,
        span: rayc_lexical::tree::RelativeSpan,
        ty: qbice::storage::intern::Interned<rayc_type::ty::Ty>,
    ) -> IRExprID {
        self.emit_expression(IRExpr::new(IRExprKind::Tuple(Tuple::new(Vec::new())), span, ty))
    }
}
