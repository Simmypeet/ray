use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, call::Call as IrCall};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, call::Call},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Call> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        call: &Call,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        let arguments = call
            .arguments()
            .iter()
            .map(|argument| self.lower_expression_by_id(typed_function, *argument))
            .collect();
        self.emit_expression(Expression::new(
            ExpressionKind::Call(IrCall::new(call.function_id(), arguments)),
            span,
            ty,
        ))
    }
}
