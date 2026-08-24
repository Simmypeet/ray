use std::io::Write;

use rayc_ir::{expression::call::Call, function::Function};
use rayc_mono::MonoFunction;

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Call> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Call>,
        _function: &Function,
        mono_function: &MonoFunction,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let call = expression.node();
        let callee = ctx.instantiate_call(mono_function, call.function_id(), call.subst());
        let name = ctx.get_def_name(callee.def_id()).await;
        let subst_id = crate::context::instantiation::MonoFunctionSubstID::for_function(&callee);
        write!(self, "{}(", Identifier::def(&name, subst_id))?;
        for (index, argument) in call.arguments().iter().enumerate() {
            if index != 0 {
                write!(self, ", ")?;
            }
            write!(self, "{}", Identifier::expr(*argument))?;
        }
        write!(self, ")")
    }
}
