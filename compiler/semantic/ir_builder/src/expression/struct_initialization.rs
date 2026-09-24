use rayc_hash::FxHashMap;
use rayc_ir::ir_expr::{IRExpr, IRExprKind, struct_initialization::StructInitialization};
use rayc_typed_ast::typed_expr::struct_initialization::StructInitialization as TypedStructInitialization;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a TypedStructInitialization>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a TypedStructInitialization>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let ty = typed_expression.ty().clone();
        let struct_initialization = expression.node();
        let struct_id = struct_initialization.struct_id();
        let mut initializers = FxHashMap::default();
        for init in struct_initialization.initializers() {
            let value = self.lower_rvalue_by_id(context, init.expression()).await;
            initializers.insert(init.field(), value);
        }

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::StructInitialization(StructInitialization::new(struct_id, initializers)),
            span,
            ty,
        )))
    }
}
