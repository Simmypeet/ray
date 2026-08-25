use rayc_ir::ir_expr::{IRExpr, IRExprID};
use rayc_typed_ast::typed_expr::errored::Errored;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a Errored>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Errored>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        self.emit_expression(IRExpr::new_error(span, ty))
    }
}
