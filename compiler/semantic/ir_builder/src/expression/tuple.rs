use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID, ExpressionKind, tuple::Tuple as IrTuple};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, tuple::Tuple},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Tuple> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        tuple: &Tuple,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        let elements = tuple
            .elements()
            .iter()
            .map(|element| self.lower_expression_by_id(typed_function, *element))
            .collect();
        self.emit_expression(Expression::new(
            ExpressionKind::Tuple(IrTuple::new(elements)),
            span,
            ty,
        ))
    }
}
