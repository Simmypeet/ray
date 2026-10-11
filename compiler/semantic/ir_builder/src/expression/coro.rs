use rayc_hash::FxHashMap;
use rayc_ir::ir_expr::{
    IRExpr, IRExprKind, coroutine::Coroutine, ref_of::RefOf,
    struct_initialization::StructInitialization,
};
use rayc_semantic_element::struct_body::get_struct_body;
use rayc_typed_ast::typed_expr::coro::Coro;

use crate::{
    builder::Builder,
    context::LoweringContext,
    expression::{Lower, LoweredExpression, TypedExprWithID},
};

impl<'a> Lower<TypedExprWithID<&'a Coro>> for Builder {
    async fn lower(
        &mut self,
        context: &LoweringContext<'_>,
        expression: TypedExprWithID<&'a Coro>,
    ) -> LoweredExpression {
        let typed_expression = context.expression(expression.id());
        let span = typed_expression.span();
        let coro = expression.node();
        let coroutine_ty = coro.coroutine_ty().clone();

        // The continuation is `Continuation[(), &mut <coroutine type>]`: it
        // only refers to the coroutine.
        let continuation_ty = typed_expression.ty().clone();
        let continuation = continuation_ty
            .as_struct_view()
            .expect("a `coro` expression always has a `Continuation` type");
        let storage_ref_ty = continuation.args()[1].clone();

        // The arguments are evaluated from left to right, as for a call.
        let mut arguments = Vec::with_capacity(coro.arguments().len());
        for argument in coro.arguments() {
            arguments.push(self.lower_rvalue_by_id(context, *argument).await);
        }

        // Build the coroutine into hidden storage that lives as long as the
        // enclosing block, which pins it: nothing names the storage, so
        // nothing can move the coroutine out of it.
        let storage = self.create_coroutine_storage(coroutine_ty.clone(), span);
        let storage_address = self.variable_address(storage);
        let coroutine = self.emit_expression(IRExpr::new(
            IRExprKind::Coroutine(Coroutine::new(arguments)),
            span,
            coroutine_ty,
        ));
        self.emit_store(storage_address.clone(), coroutine, span);

        // The continuation borrows the storage mutably, which is an ordinary
        // loan for the borrow checker.
        let storage_ref = self.emit_expression(IRExpr::new(
            IRExprKind::RefOf(RefOf::new(storage_address)),
            span,
            storage_ref_ty,
        ));

        // Wrap the reference in the continuation, whose only field holds it.
        let continuation_id = continuation.symbol_id();
        let body = self.engine().get_struct_body(continuation_id).await;
        let (coroutine_field, _) =
            body.iter().next().expect("`core.Continuation` should have a field");
        let initializers = FxHashMap::from_iter([(coroutine_field, storage_ref)]);

        LoweredExpression::RValue(self.emit_expression(IRExpr::new(
            IRExprKind::StructInitialization(StructInitialization::new(
                continuation_id,
                initializers,
            )),
            span,
            continuation_ty,
        )))
    }
}
