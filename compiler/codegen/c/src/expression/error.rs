use rayc_ir::function::Function;
use rayc_mono::MonoFunction;

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

#[derive(Debug, Clone, Copy)]
pub(super) struct Error;

impl WriteExpression<Error> for Writer<'_> {
    async fn write_expression(
        &mut self,
        _expression: ExpressionWithID<Error>,
        _function: &Function,
        _mono_function: &MonoFunction,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        panic!("error expression reached codegen, this should have been caught earlier")
    }
}
