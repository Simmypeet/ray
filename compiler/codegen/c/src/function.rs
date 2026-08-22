use std::io::Write;

use crate::{
    context::{Context, instantiation::CDefID},
    function_ctx::FunctionCtx,
    writer::Writer,
};

impl Writer<'_> {
    pub async fn generate_function_definition(
        &mut self,
        cdef_id: CDefID,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let def_id = ctx.get_cdef_decl(cdef_id).def_id();
        let function = ctx.get_typed_ast(def_id).await;

        ctx.write_cdef_decl(cdef_id, self).await?;
        write!(self, " ")?;

        let function_ctx = FunctionCtx::new(function);
        self.write_braced_block(async |writer| {
            for statement in function_ctx.statements() {
                writer.generate_statement(statement, &function_ctx, ctx).await?;
            }

            Ok(())
        })
        .await
    }
}
