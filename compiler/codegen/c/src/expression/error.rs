use rayc_ir::function::Function;

use super::{ExpressionWithID, WriteExpression};
use crate::{context::Context, writer::Writer};

#[derive(Debug, Clone, Copy)]
pub(super) struct Error;

impl WriteExpression<Error> for Writer<'_> {
    async fn write_expression(
        &mut self,
        _expression: ExpressionWithID<Error>,
        _function: &Function,
        _ctx: &mut Context,
    ) -> std::io::Result<()> {
        panic!("error expression reached codegen, this should have been caught earlier")
    }
}
