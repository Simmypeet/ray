use std::io::Write;

use crate::{
    context::{Context, instantiation::CDefID},
    writer::Writer,
};

impl Writer<'_> {
    pub async fn generate_function_definition(
        &mut self,
        cdef_id: CDefID,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let def_id = ctx.get_cdef_decl(cdef_id).def_id();
        let function = ctx.get_ir(def_id).await;

        ctx.write_cdef_decl(cdef_id, self).await?;
        write!(self, " ")?;
        self.write_ir_function_body(&function, ctx).await
    }
}
