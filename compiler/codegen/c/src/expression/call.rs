use std::io::Write;

use rayc_ir::ir_expr::call::{Call, CallTarget};
use rayc_symbol::symbol_kind::SymbolKind;

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
        let mut extern_void = false;
        match call.target() {
            CallTarget::UnresolvedInstanceAssociated { .. } => todo!(),
            CallTarget::Direct { function_id, subst } => {
                let callee = function.instantiate_call(*function_id, subst, ctx);
                let name = ctx.get_def_name(callee.def_id()).await;
                if ctx.get_symbol_kind(callee.def_id()).await == SymbolKind::ExternDef {
                    extern_void = matches!(
                        ctx.get_extern_return(&callee).await,
                        crate::c_ty::CAbiReturn::Void
                    );
                    if extern_void {
                        write!(self, "(")?;
                    }
                    write!(self, "{}(", &*name)?;
                } else {
                    let subst_id =
                        crate::context::instantiation::MonoFunctionSubstID::for_function(&callee);
                    write!(self, "{}(", Identifier::def(&name, subst_id))?;
                }
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
        write!(self, ")")?;
        if extern_void {
            let unit = ctx.get_unit_tuple_id();
            write!(self, ", (")?;
            ctx.write_ctuple_t(unit, self)?;
            write!(self, "){{0}})")?;
        }
        Ok(())
    }
}
