use rayc_ir::address::Address;
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind};

use crate::{builder::Builder, context::LoweringContext};

mod deref;
mod identifier;
mod paren;
mod tuple_index;

pub trait LowerAddress<S> {
    fn lower_address(&mut self, context: &LoweringContext<'_>, expression: &S) -> Address;
}

impl Builder {
    pub fn lower_address_by_id(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> Address {
        let expression = context.expression(expression_id);

        match expression.kind() {
            TypedExprKind::Identifier(identifier) => self.lower_address(context, identifier),
            TypedExprKind::TupleIndex(tuple_index) => self.lower_address(context, tuple_index),
            TypedExprKind::Deref(deref) => self.lower_address(context, deref),
            TypedExprKind::Paren(paren) => self.lower_address(context, paren),
            TypedExprKind::Literal(_)
            | TypedExprKind::Tuple(_)
            | TypedExprKind::Call(_)
            | TypedExprKind::Closure(_)
            | TypedExprKind::Binary(_)
            | TypedExprKind::IfElse(_)
            | TypedExprKind::RefOf(_)
            | TypedExprKind::RunWith(_)
            | TypedExprKind::Errored(_) => self.error_address(),
        }
    }
}
