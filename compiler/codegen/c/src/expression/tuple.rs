use std::io::Write;

use rayc_ir::{expression::tuple::Tuple, function::Function};

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, identifier::Identifier, writer::Writer};

impl WriteExpression<&Tuple> for Writer<'_> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<&Tuple>,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let tuple = expression.node();
        write!(self, "((")?;
        let tuple_id = ctx.unwrap_ty_as_ctuple_id(function.get_expression(expression.id()).ty());
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
