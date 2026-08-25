use super::{ExpressionWithID, WriteExpression, function_instance::FunctionInstance};
use crate::{context::Context, writer::Writer};

#[derive(Debug, Clone, Copy)]
pub(super) struct Error;

impl WriteExpression<Error> for Writer<'_> {
    async fn write_expression(
        &mut self,
        _expression: ExpressionWithID<Error>,
        _function: FunctionInstance<'_>,
        _ctx: &Context,
    ) -> std::io::Result<()> {
        panic!("error expression reached codegen, this should have been caught earlier")
    }
}
