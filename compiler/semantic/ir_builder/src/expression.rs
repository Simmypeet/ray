use rayc_ir::expression::{Expression, ExpressionID};
use rayc_typed_ast::{
    typed_function::TypedFunction as TypedFunction,
    typed_expr::{TypedExprID, TypedExprKind},
};

use crate::builder::Builder;

mod binary;
mod call;
mod deref;
mod errored;
mod identifier;
mod if_else;
mod literal;
mod paren;
mod ref_of;
mod tuple;
mod tuple_index;
mod typed_expr_id;

pub use typed_expr_id::TypedExprWithID;

pub trait LowerExpression<S> {
    fn lower_expression(&mut self, expression: S, typed_function: &TypedFunction) -> ExpressionID;
}

impl Builder {
    pub fn lower_expression_by_id(
        &mut self,
        typed_function: &TypedFunction,
        expression_id: TypedExprID,
    ) -> ExpressionID {
        let expression = typed_function.get_expression(expression_id);
        match expression.kind() {
            TypedExprKind::Identifier(identifier) => self
                .lower_expression(TypedExprWithID::new(identifier, expression_id), typed_function),
            TypedExprKind::Literal(literal) => {
                self.lower_expression(TypedExprWithID::new(literal, expression_id), typed_function)
            }
            TypedExprKind::TupleIndex(tuple_index) => self
                .lower_expression(TypedExprWithID::new(tuple_index, expression_id), typed_function),
            TypedExprKind::Tuple(tuple) => {
                self.lower_expression(TypedExprWithID::new(tuple, expression_id), typed_function)
            }
            TypedExprKind::Call(call) => {
                self.lower_expression(TypedExprWithID::new(call, expression_id), typed_function)
            }
            TypedExprKind::Lambda(_) => self
                .emit_expression(Expression::new_error(expression.span(), expression.ty().clone())),
            TypedExprKind::Binary(binary) => {
                self.lower_expression(TypedExprWithID::new(binary, expression_id), typed_function)
            }
            TypedExprKind::IfElse(if_else) => {
                self.lower_expression(TypedExprWithID::new(if_else, expression_id), typed_function)
            }
            TypedExprKind::RefOf(reference) => self
                .lower_expression(TypedExprWithID::new(reference, expression_id), typed_function),
            TypedExprKind::Deref(deref) => {
                self.lower_expression(TypedExprWithID::new(deref, expression_id), typed_function)
            }
            TypedExprKind::Paren(paren) => {
                self.lower_expression(TypedExprWithID::new(paren, expression_id), typed_function)
            }
            TypedExprKind::Errored(errored) => {
                self.lower_expression(TypedExprWithID::new(errored, expression_id), typed_function)
            }
        }
    }
}
