use std::io::Write;

use rayc_ir::expression::call::{Call, CallTarget};

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Call> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Call>,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let call = expression.node();
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let callee = function.instantiate_call(*function_id, subst, ctx);
                let name = ctx.get_def_name(callee.def_id()).await;
                let subst_id =
                    crate::context::instantiation::MonoFunctionSubstID::for_function(&callee);
                write!(self, "{}(", Identifier::def(&name, subst_id))?;
            }
            CallTarget::Lambda { callee } => {
                let callee = Identifier::expr(*callee);
                write!(
                    self,
                    "{callee}.{}({callee}.{}",
                    Identifier::lambda_call_field(),
                    Identifier::lambda_env_field()
                )?;
                if !call.arguments().is_empty() {
                    write!(self, ", ")?;
                }
            }
        }
        for (index, argument) in call.arguments().iter().enumerate() {
            if index != 0 {
                write!(self, ", ")?;
            }
            write!(self, "{}", Identifier::expr(*argument))?;
        }
        write!(self, ")")
    }
}
