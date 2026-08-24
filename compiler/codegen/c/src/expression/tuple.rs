use std::io::Write;

use rayc_ir::expression::tuple::Tuple;

use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Tuple> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Tuple>,
        function: FunctionInstance<'_>,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let tuple = expression.node();
        write!(self, "((")?;
        let ty = function.instantiate_expression_type(expression.id(), ctx);
        let tuple_id = ctx.unwrap_ty_as_ctuple_id(&ty);
        ctx.write_ctuple_t(tuple_id, self)?;
        write!(self, "){{")?;

        if tuple.elements().is_empty() {
            write!(self, "0")?;
        } else {
            for (index, element) in tuple.elements().iter().enumerate() {
                if index != 0 {
                    write!(self, ",")?;
                }
                write!(
                    self,
                    ".{} = {}",
                    Identifier::tuple_elem(index),
                    Identifier::expr(*element)
                )?;
            }
        }

        write!(self, "}})")
    }
}
