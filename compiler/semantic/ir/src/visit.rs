use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;

use crate::{
    ir_expr::{IRExpr, IRExprID},
    ir_function::FunctionID,
};

/// Receives expressions together with their function-local identity.
pub trait ExprVisitor {
    fn visit_expr(&mut self, function_id: FunctionID, expression_id: IRExprID, expression: &IRExpr);
}

impl<F> ExprVisitor for F
where
    F: FnMut(FunctionID, IRExprID, &IRExpr),
{
    fn visit_expr(
        &mut self,
        function_id: FunctionID,
        expression_id: IRExprID,
        expression: &IRExpr,
    ) {
        self(function_id, expression_id, expression);
    }
}

/// Exposes expressions with both IDs required to identify them in an IR map.
pub trait VisitExpr {
    fn visit_exprs<V: ExprVisitor>(&self, visitor: &mut V);
}

/// Receives types exposed by IR nodes.
pub trait TypeVisitor {
    fn visit_type(&mut self, ty: &Interned<Ty>);
}

impl<F> TypeVisitor for F
where
    F: FnMut(&Interned<Ty>),
{
    fn visit_type(&mut self, ty: &Interned<Ty>) { self(ty); }
}

/// Exposes the types relevant to an IR node.
pub trait VisitType {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V);
}
