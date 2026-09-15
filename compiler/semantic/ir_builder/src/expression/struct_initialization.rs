use rayc_ir::ir_expr::{IRExpr, struct_initialization::StructInitialization};
use rayc_typed_ast::typed_expr::struct_initialization::StructInitialization as TypedStructInitialization;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{LowerExpression, TypedExprWithID},
};

impl<'a> LowerExpression<TypedExprWithID<&'a TypedStructInitialization>> for Builder {
    fn lower_expression(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a TypedStructInitialization>,
    ) -> rayc_ir::ir_expr::IRExprID {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let struct_initialization = expression.node();
        let struct_id = struct_initialization.struct_id();
        let initializers = struct_initialization
            .initializers()
            .iter()
            .map(|init| (init.field(), self.lower_expression_by_id(context, init.expression())))
            .collect();

        self.emit_expression(IRExpr::new(
            StructInitialization::new(struct_id, initializers),
            span,
            ty,
        ))
    }
}
