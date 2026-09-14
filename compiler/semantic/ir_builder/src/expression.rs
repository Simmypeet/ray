use rayc_ir::ir_expr::IRExprID;
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind};

use crate::{builder::Builder, context::LoweringContext};

mod binary;
mod call;
mod closure;
mod deref;
mod errored;
mod identifier;
mod if_else;
mod literal;
mod paren;
mod ref_of;
mod run_with;
mod tuple;
mod tuple_index;
mod typed_expr_id;
mod while_loop;

pub use typed_expr_id::TypedExprWithID;

pub trait LowerExpression<S> {
    fn lower_expression(&mut self, context: &LoweringContext<'_>, expression: S) -> IRExprID;
}

impl Builder {
    pub fn lower_expression_by_id(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> IRExprID {
        let expression = context.expression(expression_id);
        match expression.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.lower_expression(context, TypedExprWithID::new(identifier, expression_id))
            }
            TypedExprKind::Literal(literal) => {
                self.lower_expression(context, TypedExprWithID::new(literal, expression_id))
            }
            TypedExprKind::TupleIndex(tuple_index) => {
                self.lower_expression(context, TypedExprWithID::new(tuple_index, expression_id))
            }
            TypedExprKind::Tuple(tuple) => {
                self.lower_expression(context, TypedExprWithID::new(tuple, expression_id))
            }
            TypedExprKind::Call(call) => {
                self.lower_expression(context, TypedExprWithID::new(call, expression_id))
            }
            TypedExprKind::Closure(lambda) => {
                self.lower_expression(context, TypedExprWithID::new(lambda, expression_id))
            }

            TypedExprKind::Binary(binary) => {
                self.lower_expression(context, TypedExprWithID::new(binary, expression_id))
            }
            TypedExprKind::IfElse(if_else) => {
                self.lower_expression(context, TypedExprWithID::new(if_else, expression_id))
            }
            TypedExprKind::While(while_loop) => {
                self.lower_expression(context, TypedExprWithID::new(while_loop, expression_id))
            }
            TypedExprKind::RefOf(reference) => {
                self.lower_expression(context, TypedExprWithID::new(reference, expression_id))
            }
            TypedExprKind::Deref(deref) => {
                self.lower_expression(context, TypedExprWithID::new(deref, expression_id))
            }
            TypedExprKind::Paren(paren) => {
                self.lower_expression(context, TypedExprWithID::new(paren, expression_id))
            }
            TypedExprKind::RunWith(run_with) => {
                self.lower_expression(context, TypedExprWithID::new(run_with, expression_id))
            }
            TypedExprKind::Errored(errored) => {
                self.lower_expression(context, TypedExprWithID::new(errored, expression_id))
            }
        }
    }
}
