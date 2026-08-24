use qbice::storage::intern::Interned;
use rayc_ir::{
    expression::{Expression, ExpressionID},
    function::Function,
};
use rayc_mono::MonoFunction;
use rayc_type::ty::Ty;

use crate::context::Context;

#[derive(Debug, Clone, Copy)]
pub struct FunctionInstance<'function> {
    function: &'function Function,
    mono_function: &'function MonoFunction,
}

impl<'function> FunctionInstance<'function> {
    pub(crate) const fn new(
        function: &'function Function,
        mono_function: &'function MonoFunction,
    ) -> Self {
        Self { function, mono_function }
    }

    pub(super) fn get_expression(self, expression_id: ExpressionID) -> &'function Expression {
        self.function.get_expression(expression_id)
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
        expression_id: ExpressionID,
        ctx: &Context,
    ) -> Interned<Ty> {
        ctx.instantiate_type(self.get_expression(expression_id).ty(), self.mono_function)
    }
}
