use rayc_lexical::tree::RelativeSpan;
use rayc_source_file::SourceElement;
use rayc_symbol::{
    GlobalSymbolID,
    core_item::{CoreItem, get_core_item},
};
use rayc_syntax::expression::{Coro as CoroSyntax, Leaf, PostfixOperator};
use rayc_type::{
    poly_var::get_enclosing_poly_var_maps,
    subst::Subst,
    ty::{Mutability, Ty, TyKind, args::Args, lifetime::Lifetime},
};
use rayc_typed_ast::typed_expr::{TypedExprID, coro::Coro, errored::Errored};

use crate::{
    bind::Bind,
    diagnostic::{CoroOperandNotCall, Diagnostic, UnsupportedCoroCallee},
    expression::call::Invocation,
    tast_builder::TAstBuilder,
};

impl Bind<CoroSyntax> for TAstBuilder {
    async fn bind(&mut self, syn: CoroSyntax) -> TypedExprID {
        let span = syn.span();
        let Some(operand) = syn.operand() else {
            return self.push_error_expression(span).await;
        };

        // The call of a path is the only operand a coroutine can be created
        // from so far. It is bound as a coroutine, not as a call.
        let last_postfix = operand.postfixes().last();
        if last_postfix.is_none()
            && let Some(Leaf::DirectCall(call)) = operand.leaf()
        {
            return self.build_path_direct_call(&call, Invocation::Coroutine { span }).await;
        }

        // Anything else is an error. The operand is not bound: binding a call
        // would introduce its effect, which a `coro` does not perform.
        let is_call = matches!(last_postfix, Some(PostfixOperator::Call(_)));
        self.reject_coro_operand(operand.span(), is_call, [], span).await
    }
}

impl TAstBuilder {
    /// Reports the operand at `operand_span` of the `coro` at `span`, which
    /// no coroutine can be created from, and returns the `coro` as an errored
    /// expression that keeps `children`.
    ///
    /// `is_call` tells whether the operand is a call whose callee is not
    /// supported yet, rather than no call at all.
    pub(super) async fn reject_coro_operand(
        &mut self,
        operand_span: RelativeSpan,
        is_call: bool,
        children: impl IntoIterator<Item = TypedExprID>,
        span: RelativeSpan,
    ) -> TypedExprID {
        self.push_diagnostic(if is_call {
            Diagnostic::UnsupportedCoroCallee(UnsupportedCoroCallee::new(operand_span))
        } else {
            Diagnostic::CoroOperandNotCall(CoroOperandNotCall::new(operand_span))
        });

        // The error was reported, so the expression has the error type
        // instead of a type that nothing would ever determine.
        let children = children.into_iter().map(Into::into).collect();
        let ty = Ty::new_error(TyKind::Star, self.engine());
        self.insert_expression(Errored::new(children), span, ty).await
    }

    /// Builds `coro function_id(arguments)`, where `call_subst` instantiates
    /// the polymorphic parameters of `function_id` and of its enclosing
    /// symbols, and the arguments were already checked against its
    /// parameters.
    ///
    /// Evaluating it runs none of the function, so it introduces no effect:
    /// the expression only performs what its arguments perform.
    pub(super) async fn build_coro(
        &mut self,
        function_id: GlobalSymbolID,
        arguments: Vec<TypedExprID>,
        call_subst: &Subst,
        span: RelativeSpan,
    ) -> TypedExprID {
        // The coroutine type is the function and its polymorphic arguments.
        // The substitution of a suspendable callee covers all of them: a
        // `def` is enclosed by no polymorphic symbol, and the substitution
        // of an instance's `def` includes the arguments of the instance.
        let poly_vars = self.engine().get_enclosing_poly_var_maps(function_id).await;
        let poly_args = poly_vars.all_poly_vars().map(|poly_var_id| {
            call_subst
                .get(&poly_var_id)
                .cloned()
                .expect("the call should instantiate every parameter of the coroutine's function")
        });
        let coroutine_ty = Ty::new_coroutine(function_id, poly_args, self.engine());

        // The coroutine is pinned to hidden storage, and the continuation
        // only refers to it. Type inference ignores lifetimes, so the
        // lifetime of that borrow is left for the borrow checker.
        let storage_ty = Ty::new_reference(
            Ty::new_lifetime(Lifetime::Erased, self.engine()),
            coroutine_ty.clone(),
            Mutability::Mutable,
            self.engine(),
        );

        // A new coroutine is always resumed with the unit value.
        let continuation_id = self.engine().get_core_item(CoreItem::ContinuationStruct).await;
        let continuation_ty = Ty::new_struct(
            continuation_id,
            Args::new([Ty::new_unit(self.engine()), storage_ty], self.engine()),
            self.engine(),
        );

        self.insert_expression(Coro::new(coroutine_ty, arguments), span, continuation_ty).await
    }
}
