use qbice::storage::intern::Interned;
use rayc_ir::expression::{
    Expression, ExpressionID, ExpressionKind, literal::Literal as IrLiteral,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, literal::Literal},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Literal> for Builder {
    fn lower_expression(
        &mut self,
        _typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        literal: &Literal,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        let literal = match literal {
            Literal::Numeric(value) => IrLiteral::Numeric(*value),
            Literal::Bool(value) => IrLiteral::Bool(*value),
        };
        self.emit_expression(Expression::new(ExpressionKind::Literal(literal), span, ty))
    }
}
