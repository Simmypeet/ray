pub use expression_with_id::ExpressionWithID;
use rayc_ir::ir_expr::{IRExprID, IRExprKind};

use self::function_instance::FunctionInstance;
use crate::{context::Context, writer::Writer};

pub mod address;
pub mod binary;
pub mod call;
pub mod error;
pub mod expression_with_id;
pub mod function_instance;
pub mod literal;
pub mod load;
pub mod make_lambda;
pub mod phi;
pub mod ref_of;
pub mod tuple;

pub trait WriteExpression<E> {
    #[expect(async_fn_in_trait)]
    async fn write_expression(
        &mut self,
        expression: ExpressionWithID<E>,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()>;
}

impl Writer<'_> {
    pub(crate) async fn write_expression_value(
        &mut self,
        expression_id: IRExprID,
        function: FunctionInstance<'_>,
        ctx: &Context,
    ) -> std::io::Result<()> {
        let expression = function.get_expression(expression_id);
        match expression.kind() {
            IRExprKind::Error => {
                self.write_expression(
                    ExpressionWithID::new(error::Error, expression_id),
                    function,
                    ctx,
                )
                .await
            }
            IRExprKind::Literal(literal) => {
                self.write_expression(ExpressionWithID::new(literal, expression_id), function, ctx)
                    .await
            }
            IRExprKind::RefOf(reference) => {
                self.write_expression(
                    ExpressionWithID::new(reference, expression_id),
                    function,
                    ctx,
                )
                .await
            }
            IRExprKind::Load(load) => {
                self.write_expression(ExpressionWithID::new(load, expression_id), function, ctx)
                    .await
            }
            IRExprKind::Phi(phi) => {
                self.write_expression(ExpressionWithID::new(phi, expression_id), function, ctx)
                    .await
            }
            IRExprKind::Binary(binary) => {
                self.write_expression(ExpressionWithID::new(binary, expression_id), function, ctx)
                    .await
            }
            IRExprKind::Call(call) => {
                self.write_expression(ExpressionWithID::new(call, expression_id), function, ctx)
                    .await
            }
            IRExprKind::Tuple(tuple) => {
                self.write_expression(ExpressionWithID::new(tuple, expression_id), function, ctx)
                    .await
            }
            IRExprKind::MakeLambda(lambda) => {
                self.write_expression(ExpressionWithID::new(lambda, expression_id), function, ctx)
                    .await
            }
        }
    }
}
