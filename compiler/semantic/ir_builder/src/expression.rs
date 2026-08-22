use rayc_ir::expression::ExpressionID;
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
    fn lower_expression(&mut self, expression: S, typed_function: &TypedFunction) -> ExpressionID;
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TypedExprWithID<E> {
    node: E,
    id: TypedExprID,
}

impl<E> TypedExprWithID<E> {
    pub(crate) const fn new(node: E, id: TypedExprID) -> Self { Self { node, id } }

    pub(crate) const fn id(&self) -> TypedExprID { self.id }

    pub(crate) const fn node(&self) -> E
    where
        E: Copy,
    {
        self.node
    }
}

impl Builder {
    pub(crate) fn lower_expression_by_id(
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
            TypedExprKind::Binary(binary) => {
                self.lower_expression(TypedExprWithID::new(binary, expression_id), typed_function)
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
