use qbice::storage::intern::Interned;
use rayc_ir::expression::{Expression, ExpressionID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, errored::Errored},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Errored> for Builder {
    fn lower_expression(
        &mut self,
        _typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        _errored: &Errored,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID {
        self.emit_expression(Expression::new_error(span, ty))
    }
}
