use rayc_ir::{
    cfg::{Conditional, Terminator},
    expression::{Expression, ExpressionID, ExpressionKind, phi::Phi},
};
use rayc_typed_ast::{typed_function::TypedFunction as TypedFunction, typed_expr::if_else::IfElse};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a IfElse>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a IfElse>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let if_else = expression.node();

        let condition = self.lower_expression_by_id(typed_function, if_else.condition());
        let then_block = self.create_block();
        let else_block = self.create_block();
        let merge_block = self.create_block();
        self.terminate(Terminator::Conditional(Conditional::new(
            condition, then_block, else_block,
        )));

        self.select_block(then_block);
        let then_expression =
            self.lower_expression_by_id(typed_function, if_else.then_expression());
        let then_predecessor = self.jump_to(merge_block);

        self.select_block(else_block);
        let else_expression =
            self.lower_expression_by_id(typed_function, if_else.else_expression());
        let else_predecessor = self.jump_to(merge_block);

        self.select_block(merge_block);
        let incoming = [(then_predecessor, then_expression), (else_predecessor, else_expression)]
            .into_iter()
            .collect();
        self.emit_expression(Expression::new(ExpressionKind::Phi(Phi::new(incoming)), span, ty))
    }
}
