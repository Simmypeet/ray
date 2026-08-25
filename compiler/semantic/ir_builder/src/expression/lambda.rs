use rayc_ir::expression::{
    Expression, ExpressionID, ExpressionKind, make_lambda::MakeLambda, ref_of::RefOf as IrRefOf,
};
use rayc_typed_ast::typed_expr::lambda::Lambda;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Lambda>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Lambda>,
    ) -> ExpressionID {
        let typed_expression = context.expression(expression.id());
        let lambda = expression.node();
        let function_id = self.lower_lambda_function(context, lambda.function_id());
        let captures = context
            .capture_plan(lambda.function_id())
            .captures()
            .map(|(_, requirement)| {
                let address = self.source_address(requirement.source());
                let ty =
                    self.pointer_ty(requirement.pointee_ty().clone(), requirement.mutability());
                self.emit_expression(Expression::new(
                    ExpressionKind::RefOf(IrRefOf::new(address)),
                    requirement.span(),
                    ty,
                ))
            })
            .collect();

        self.emit_expression(Expression::new(
            ExpressionKind::MakeLambda(MakeLambda::new(function_id, captures)),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}
