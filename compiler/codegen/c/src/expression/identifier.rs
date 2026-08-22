use std::io::Write;

use rayc_typed_ast::{
    name_binding::Source,
    typed_expr::{TypedExprID, identifier::Identifier},
};

use crate::{context::Context, expression::Generate, function_ctx::FunctionCtx, writer::Writer};

impl Generate<Identifier> for Writer<'_> {
    async fn generate(
        &mut self,
        expr: &Identifier,
        _: TypedExprID,
        function_ctx: &FunctionCtx,
        _: &mut Context,
    ) -> std::io::Result<()> {
        match function_ctx.get_name_binding(expr.name_binding()).source() {
            Source::Variable(id) => {
                write!(self, "ray_var_{:X}", id.index())
            }
            Source::Parameter(id) => {
                write!(self, "ray_param_{:X}", id.index())
            }
        }
    }
}
