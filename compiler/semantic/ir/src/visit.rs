use qbice::storage::intern::Interned;
use rayc_type::ty::Ty;

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
