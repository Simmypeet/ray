use std::io::Write;

use rayc_mono::MonoFunction;

use crate::{context::Context, writer::Writer};

impl Writer<'_> {
    pub async fn generate_function_definition(
        &mut self,
        mono_function: &MonoFunction,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let function = ctx.get_ir(mono_function.def_id()).await;

        ctx.write_mono_function_decl(mono_function, self).await?;
        write!(self, " ")?;
        self.write_ir_function_body(&function, mono_function, ctx).await
    }
}
