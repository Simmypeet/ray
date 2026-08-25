use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_expr::{IRExpr, IRExprID},
    ir_function::{FunctionID, IRFunction, IRFunctionMap},
    ir_lambda::IRLambdaContext,
};
use rayc_mono::{MonoFunction, MonoFunctionKind};
use rayc_type::ty::Ty;

use crate::context::Context;

#[derive(Debug, Clone, Copy)]
pub struct FunctionInstance<'function> {
    functions: &'function IRFunctionMap,
    function_id: FunctionID,
    mono_function: &'function MonoFunction,
}

impl<'function> FunctionInstance<'function> {
    pub(crate) const fn new(
        functions: &'function IRFunctionMap,
        function_id: FunctionID,
        mono_function: &'function MonoFunction,
    ) -> Self {
        Self { functions, function_id, mono_function }
    }

    pub(crate) fn function(self) -> &'function IRFunction {
        self.functions.get_function(self.function_id)
    }

    pub(crate) const fn mono_function(self) -> &'function MonoFunction { self.mono_function }

    pub(crate) fn lambda_context(self) -> &'function IRLambdaContext {
        self.function().context().assert_as_lambda_context()
    }

    pub(crate) fn target_lambda_context(
        self,
        function_id: FunctionID,
    ) -> &'function IRLambdaContext {
        self.functions.get_function(function_id).context().assert_as_lambda_context()
    }

    pub(crate) fn target_lambda(self, function_id: FunctionID) -> MonoFunction {
        MonoFunction::new_lambda(self.mono_function, function_id)
    }

    pub(crate) fn is_lambda(self) -> bool {
        match self.mono_function.kind() {
            MonoFunctionKind::Def => false,
            MonoFunctionKind::ExternDef => {
                panic!("compiler-internal invariant violation: extern function has no IR body")
            }
            MonoFunctionKind::Lambda(function_id) => {
                assert_eq!(
                    function_id, self.function_id,
                    "compiler-internal invariant violation: C function instance selects the wrong \
                     IR lambda"
                );
                true
            }
        }
    }

    pub(super) fn get_expression(self, expression_id: IRExprID) -> &'function IRExpr {
        self.function().get_expression(expression_id)
    }

    pub(super) fn instantiate_call(
        self,
        def_id: rayc_symbol::GlobalSymbolID,
        subst: &rayc_type::subst::Subst,
        ctx: &Context,
    ) -> MonoFunction {
        ctx.instantiate_call(self.mono_function, def_id, subst)
    }

    pub(super) fn instantiate_expression_type(
        self,
        expression_id: IRExprID,
        ctx: &Context,
    ) -> Interned<Ty> {
        ctx.instantiate_type(self.get_expression(expression_id).ty(), self.mono_function)
    }
}
