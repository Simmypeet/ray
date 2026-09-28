use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;

use crate::{
    ir_expr::{IRExpr, IRExprID},
    ir_function::FunctionID,
    ir_lambda::CaptureMapID,
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

/// Where a visited type is stored in an
/// [`IRFunctionMap`](crate::ir_function::IRFunctionMap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeSite {
    /// A type in a capture layout: the type of a captured binding or the
    /// lifetime of a by-reference capture.
    ///
    /// A capture layout may be shared by several nested functions, such as
    /// the operation handlers of one handler record.
    Capture(CaptureMapID),

    /// A type in the signature of a nested function: the type of a
    /// parameter, the return type or the effect.
    ///
    /// The definition function stores no signature types; its signature is
    /// the one declared by the definition.
    Signature(FunctionID),

    /// Any other type owned by a function, such as the type of a variable or
    /// of an expression, or a dictionary used by an instruction.
    Body(FunctionID),
}

/// Receives types exposed by IR nodes, together with where they are stored.
pub trait TypeVisitor {
    fn visit_type(&mut self, ty: &Interned<Ty>, site: TypeSite);
}

impl<F> TypeVisitor for F
where
    F: FnMut(&Interned<Ty>, TypeSite),
{
    fn visit_type(&mut self, ty: &Interned<Ty>, site: TypeSite) { self(ty, site); }
}

/// Exposes every type stored in an IR node.
///
/// The node reports every type at `site`, the site of the node itself.
pub trait VisitType {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V);
}

/// Receives mutable access to the types exposed by IR nodes, together with
/// where they are stored, for rewriting them in place.
pub trait TypeVisitorMut {
    fn visit_type_mut(&mut self, ty: &mut Interned<Ty>, site: TypeSite);
}

impl<F> TypeVisitorMut for F
where
    F: FnMut(&mut Interned<Ty>, TypeSite),
{
    fn visit_type_mut(&mut self, ty: &mut Interned<Ty>, site: TypeSite) { self(ty, site); }
}

/// Exposes every type stored in an IR node for rewriting in place.
///
/// This visits the same types as [`VisitType`], so that rewriting every
/// visited type rewrites the whole node. The node reports every type at
/// `site`, the site of the node itself.
pub trait VisitTypeMut {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V);
}
