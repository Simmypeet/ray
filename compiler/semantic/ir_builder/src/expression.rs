use rayc_ir::{
    address::Address,
    ir_expr::{IRExpr, IRExprID, IRExprKind, load::Load},
};
use rayc_typed_ast::typed_expr::{TypedExprID, TypedExprKind};

use crate::{builder::Builder, context::LoweringContext};

mod binary;
mod call;
mod closure;
mod deref;
mod errored;
mod field;
mod identifier;
mod if_else;
mod literal;
mod r#move;
mod paren;
mod ref_of;
mod run_with;
mod statement_block;
mod struct_initialization;
mod tuple;
mod tuple_index;
mod typed_expr_id;
mod while_loop;

pub use typed_expr_id::TypedExprWithID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum LoweredExpression {
    LValue(Address),
    RValue(IRExprID),
}

/// A trait for lowering typed expressions into IR expressions. Typically, this
/// is implemented for the [`Builder`] struct with [`TypedExprWithID`] as the
/// type parameter.
///
/// # L-Value vs R-Value Lowering Policy
///
/// Some expression nodes can be lowered into either an L-value or an R-value.
/// Those expressions must always be lowered into an L-value.
pub trait Lower<S> {
    #[allow(async_fn_in_trait)]
    async fn lower(&mut self, context: &LoweringContext<'_>, expression: S) -> LoweredExpression;
}

impl Builder {
    /// Every recursive lowering passes through here, so this is where the
    /// recursion is boxed.
    pub async fn lower_by_id(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> LoweredExpression {
        Box::pin(self.lower_expression_kind(context, expression_id)).await
    }

    async fn lower_expression_kind(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> LoweredExpression {
        let expression = context.expression(expression_id);
        match expression.kind() {
            TypedExprKind::Identifier(identifier) => {
                self.lower(context, TypedExprWithID::new(identifier, expression_id)).await
            }
            TypedExprKind::Literal(literal) => {
                self.lower(context, TypedExprWithID::new(literal, expression_id)).await
            }
            TypedExprKind::TupleIndex(tuple_index) => {
                self.lower(context, TypedExprWithID::new(tuple_index, expression_id)).await
            }
            TypedExprKind::FieldAccess(field) => {
                self.lower(context, TypedExprWithID::new(field, expression_id)).await
            }
            TypedExprKind::Tuple(tuple) => {
                self.lower(context, TypedExprWithID::new(tuple, expression_id)).await
            }
            TypedExprKind::Call(call) => {
                self.lower(context, TypedExprWithID::new(call, expression_id)).await
            }
            TypedExprKind::Closure(lambda) => {
                self.lower(context, TypedExprWithID::new(lambda, expression_id)).await
            }
            TypedExprKind::Binary(binary) => {
                self.lower(context, TypedExprWithID::new(binary, expression_id)).await
            }
            TypedExprKind::IfElse(if_else) => {
                self.lower(context, TypedExprWithID::new(if_else, expression_id)).await
            }
            TypedExprKind::While(while_loop) => {
                self.lower(context, TypedExprWithID::new(while_loop, expression_id)).await
            }
            TypedExprKind::StatementBlock(block) => {
                self.lower(context, TypedExprWithID::new(block, expression_id)).await
            }
            TypedExprKind::RefOf(reference) => {
                self.lower(context, TypedExprWithID::new(reference, expression_id)).await
            }
            TypedExprKind::Deref(deref) => {
                self.lower(context, TypedExprWithID::new(deref, expression_id)).await
            }
            TypedExprKind::Move(move_expr) => {
                self.lower(context, TypedExprWithID::new(move_expr, expression_id)).await
            }
            TypedExprKind::Paren(paren) => {
                self.lower(context, TypedExprWithID::new(paren, expression_id)).await
            }
            TypedExprKind::RunWith(run_with) => {
                self.lower(context, TypedExprWithID::new(run_with, expression_id)).await
            }
            TypedExprKind::StructInitialization(st) => {
                self.lower(context, TypedExprWithID::new(st, expression_id)).await
            }
            TypedExprKind::Errored(errored) => {
                self.lower(context, TypedExprWithID::new(errored, expression_id)).await
            }
        }
    }

    /// Lowers an expression into an R-Value. If the expression is an L-Value,
    /// the address will be loaded, generating an R-Value.
    pub async fn lower_rvalue_by_id(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> IRExprID {
        let lowered = self.lower_by_id(context, expression_id).await;
        self.lowered_expression_to_rvalue(context, expression_id, lowered)
    }

    /// Lowers an expression into an L-Value. If the expression is an R-Value,
    /// an error address will be returned.
    pub async fn lower_lvalue_by_id(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
    ) -> Address {
        match self.lower_by_id(context, expression_id).await {
            LoweredExpression::LValue(address) => address,
            LoweredExpression::RValue(_) => self.error_address(),
        }
    }

    /// Lowers an expression into an address. If the expression is an R-Value,
    /// a temporary will be created to store the value, and the address of the
    /// temporary will be returned.
    pub(crate) fn lower_to_address_or_temporary(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
        lowered: LoweredExpression,
    ) -> Address {
        match lowered {
            LoweredExpression::LValue(address) => address,
            LoweredExpression::RValue(value) => {
                // A computed operand needs storage before a projection can extend its address.
                let typed_expression = context.expression(expression_id);
                let temporary =
                    self.create_temporary(typed_expression.ty().clone(), typed_expression.span());
                let address = self.variable_address(temporary);
                self.emit_store(address.clone(), value, typed_expression.span());
                address
            }
        }
    }

    pub(crate) fn lowered_expression_to_rvalue(
        &mut self,
        context: &LoweringContext<'_>,
        expression_id: TypedExprID,
        lowered: LoweredExpression,
    ) -> IRExprID {
        match lowered {
            LoweredExpression::LValue(address) => {
                let typed_expression = context.expression(expression_id);
                self.emit_expression(IRExpr::new(
                    IRExprKind::Load(Load::new(address)),
                    typed_expression.span(),
                    typed_expression.ty().clone(),
                ))
            }
            LoweredExpression::RValue(value) => value,
        }
    }
}
