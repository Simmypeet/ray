use std::io::Write;

use rayc_ir::expression::call::Call;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Call> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Call>,
        function: FunctionInstance<'_>,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let call = expression.node();
        let callee = function.instantiate_call(call, ctx);
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
