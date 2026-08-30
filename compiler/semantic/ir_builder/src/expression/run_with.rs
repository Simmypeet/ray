use rayc_ir::ir_expr::{IRExpr, IRExprID, IRExprKind, tuple::Tuple};
use rayc_typed_ast::typed_expr::run_with::RunWith;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a RunWith>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a RunWith>,
    ) -> IRExprID {
        let typed_expression = context.expression(expression.id());
        self.emit_expression(IRExpr::new(
            IRExprKind::Tuple(Tuple::new(Vec::new())),
            typed_expression.span(),
            typed_expression.ty().clone(),
        ))
    }
}
