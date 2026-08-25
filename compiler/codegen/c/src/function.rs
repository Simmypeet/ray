use std::io::Write;

use rayc_mono::{MonoFunction, MonoFunctionKind};

use crate::{context::Context, expression::function_instance::FunctionInstance, writer::Writer};

impl Writer<'_> {
    pub async fn generate_function_definition(
        &mut self,
        mono_function: &MonoFunction,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let functions = ctx.get_ir(mono_function.def_id()).await;
        let function_id = match mono_function.kind() {
            MonoFunctionKind::Def => functions.root_id(),
            MonoFunctionKind::ExternDef => {
                panic!("compiler-internal invariant violation: extern function has no definition")
            }
            MonoFunctionKind::Lambda(function_id) => function_id,
        };
        let function = FunctionInstance::new(&functions, function_id, mono_function);

        ctx.write_mono_function_decl(mono_function, self).await?;
        write!(self, " ")?;
        self.write_ir_function_body(function, ctx).await
    }
}
