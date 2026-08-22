use rayc_ir::{
    expression::{ExpressionID, ExpressionKind},
    function::Function,
};

use crate::{context::Context, writer::Writer};

mod address;
mod binary;
mod call;
mod error;
mod literal;
mod load;
mod phi;
mod ref_of;
mod tuple;

#[derive(Debug, Clone, Copy)]
pub struct ExpressionWithID<E> {
    node: E,
    id: ExpressionID,
}

impl<E> ExpressionWithID<E> {
    pub const fn new(node: E, id: ExpressionID) -> Self { Self { node, id } }

    pub const fn id(&self) -> ExpressionID { self.id }

    pub const fn node(&self) -> E
    where
        E: Copy,
    {
        self.node
    }
}

pub trait WriteExpression<E> {
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<E>,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()>;
}

impl Writer<'_> {
    pub(crate) async fn write_expression_value(
        &mut self,
        expression_id: ExpressionID,
        function: &Function,
        ctx: &mut Context,
    ) -> std::io::Result<()> {
        let expression = function.get_expression(expression_id);
        match expression.kind() {
            ExpressionKind::Error => {
                self.write_expression(
                    ExpressionWithID::new(error::Error, expression_id),
                    function,
                    ctx,
                )
                .await
            }
            ExpressionKind::Literal(literal) => {
                self.write_expression(ExpressionWithID::new(literal, expression_id), function, ctx)
                    .await
            }
            ExpressionKind::RefOf(reference) => {
                self.write_expression(
                    ExpressionWithID::new(reference, expression_id),
                    function,
                    ctx,
                )
                .await
            }
            ExpressionKind::Load(load) => {
                self.write_expression(ExpressionWithID::new(load, expression_id), function, ctx)
                    .await
            }
            ExpressionKind::Phi(phi) => {
                self.write_expression(ExpressionWithID::new(phi, expression_id), function, ctx)
                    .await
            }
            ExpressionKind::Binary(binary) => {
                self.write_expression(ExpressionWithID::new(binary, expression_id), function, ctx)
                    .await
            }
            ExpressionKind::Call(call) => {
                self.write_expression(ExpressionWithID::new(call, expression_id), function, ctx)
                    .await
            }
            ExpressionKind::Tuple(tuple) => {
                self.write_expression(ExpressionWithID::new(tuple, expression_id), function, ctx)
                    .await
            }
        }
    }
}
