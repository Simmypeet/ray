use rayc_ir::address::Address;
use rayc_typed_ast::{
    function::Function as TypedFunction,
    typed_expr::{TypedExprID, TypedExprKind},
};

use crate::builder::Builder;

mod deref;
mod identifier;
mod paren;
mod tuple_index;

pub trait LowerAddress<S> {
    fn lower_address(&mut self, typed_function: &TypedFunction, expression: &S) -> Address;
}

impl Builder {
    pub fn lower_address_by_id(
        &mut self,
        typed_function: &TypedFunction,
        expression_id: TypedExprID,
    ) -> Address {
        let expression = typed_function.get_expression(expression_id);

        match expression.kind() {
            TypedExprKind::Identifier(identifier) => self.lower_address(typed_function, identifier),
            TypedExprKind::TupleIndex(tuple_index) => {
                self.lower_address(typed_function, tuple_index)
            }
            TypedExprKind::Deref(deref) => self.lower_address(typed_function, deref),
            TypedExprKind::Paren(paren) => self.lower_address(typed_function, paren),
            TypedExprKind::Literal(_)
            | TypedExprKind::Tuple(_)
            | TypedExprKind::Call(_)
            | TypedExprKind::Binary(_)
            | TypedExprKind::RefOf(_)
            | TypedExprKind::Errored(_) => self.error_address(),
        }
    }
}
