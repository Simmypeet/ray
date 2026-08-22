use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, ref_of::RefOf as IrRefOf};
use rayc_typed_ast::{function::Function as TypedFunction, typed_expr::ref_of::RefOf};

use crate::{
    builder::Builder,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a RefOf>> for Builder {
    fn lower_expression(
        &mut self,
        expression: TypedExprWithID<&'a RefOf>,
        typed_function: &TypedFunction,
    ) -> ExpressionID {
        let typed_expression = typed_function.get_expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let reference = expression.node();
        let address = self.lower_address_by_id(typed_function, reference.pointee());
        self.emit_expression(Expression::new(
            ExpressionKind::RefOf(IrRefOf::new(address)),
            span,
            ty,
        ))
    }
}
