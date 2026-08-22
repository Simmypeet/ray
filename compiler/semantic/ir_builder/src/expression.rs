use qbice::storage::intern::Interned;
use rayc_ir::expression::ExpressionID;
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, TypedExprKind},
};

use crate::builder::Builder;

mod binary;
mod call;
mod deref;
mod errored;
mod identifier;
mod literal;
mod paren;
mod ref_of;
mod tuple;
mod tuple_index;

pub(crate) trait LowerExpression<S> {
    fn lower_expression(
        &mut self,
        typed_function: &TypedFunction,
        expression_id: TypedExprID,
        expression: &S,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> ExpressionID;
}

impl Builder {
    pub(crate) fn lower_expression_by_id(
        &mut self,
        typed_function: &TypedFunction,
        expression_id: TypedExprID,
    ) -> ExpressionID {
        let expression = typed_function.get_expression(expression_id);
        let span = expression.span();
        let ty = expression.ty().clone();

        match expression.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.lower_expression(typed_function, expression_id, identifier, span, ty)
            }
            TypedExprKind::Literal(literal) => {
                self.lower_expression(typed_function, expression_id, literal, span, ty)
            }
            TypedExprKind::TupleIndex(tuple_index) => {
                self.lower_expression(typed_function, expression_id, tuple_index, span, ty)
            }
            TypedExprKind::Tuple(tuple) => {
                self.lower_expression(typed_function, expression_id, tuple, span, ty)
            }
            TypedExprKind::Call(call) => {
                self.lower_expression(typed_function, expression_id, call, span, ty)
            }
            TypedExprKind::Binary(binary) => {
                self.lower_expression(typed_function, expression_id, binary, span, ty)
            }
            TypedExprKind::RefOf(reference) => {
                self.lower_expression(typed_function, expression_id, reference, span, ty)
            }
            TypedExprKind::Deref(deref) => {
                self.lower_expression(typed_function, expression_id, deref, span, ty)
            }
            TypedExprKind::Paren(paren) => {
                self.lower_expression(typed_function, expression_id, paren, span, ty)
            }
            TypedExprKind::Errored(errored) => {
                self.lower_expression(typed_function, expression_id, errored, span, ty)
            }
        }
    }
}
