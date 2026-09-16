use rayc_typed_ast::typed_expr::identifier::Identifier;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Identifier>> for Builder {
    fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Identifier>,
    ) -> LoweredExpression {
        let source = context.name_binding_source(expression.node().name_binding());
        LoweredExpression::LValue(self.source_address(source))
    }
}
