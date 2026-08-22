use qbice::storage::intern::Interned;
use rayc_ir::expression::ExpressionID;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, paren::Paren},
};

use crate::{builder::Builder, expression::LowerExpression};

impl LowerExpression<Paren> for Builder {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        _expression_id: TypedExprID,
        paren: &Paren,
        _span: RelativeSpan,
        _ty: Interned<Ty>,
    ) -> ExpressionID {
        self.lower_expression_by_id(typed_function, paren.expression())
    }
}
