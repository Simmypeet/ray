use derive_more::From;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    ir_expr::{
        binary::Binary, call::Call, closure::Closure, handle::Handle, literal::Literal, load::Load,
        perform::Perform, phi::Phi, ref_of::RefOf, ref_to_pointer::RefToPointer,
        struct_initialization::StructInitialization, tuple::Tuple,
    },
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
};

pub mod binary;
pub mod call;
pub mod closure;
pub mod handle;
pub mod literal;
pub mod load;
pub mod perform;
pub mod phi;
pub mod ref_of;
pub mod ref_to_pointer;
pub mod struct_initialization;
pub mod tuple;

/// Identifies an expression value stored in a function's expression arena.
pub type IRExprID = ID<IRExpr>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, From)]
pub enum IRExprKind {
    Error,
    Literal(Literal),
    RefOf(RefOf),
    RefToPointer(RefToPointer),
    Load(Load),
    Phi(Phi),
    Binary(Binary),
    Call(Call),
    Perform(Perform),
    Handle(Handle),
    Tuple(Tuple),
    Closure(Closure),
    StructInitialization(StructInitialization),
}

impl IRExprKind {
    /// Returns the phi this expression is, if it is one.
    #[must_use]
    pub const fn as_phi(&self) -> Option<&Phi> {
        match self {
            Self::Phi(phi) => Some(phi),
            Self::Error
            | Self::Literal(_)
            | Self::RefOf(_)
            | Self::RefToPointer(_)
            | Self::Load(_)
            | Self::Binary(_)
            | Self::Call(_)
            | Self::Perform(_)
            | Self::Handle(_)
            | Self::Tuple(_)
            | Self::Closure(_)
            | Self::StructInitialization(_) => None,
        }
    }

    /// Returns the phi this expression is, if it is one, for rewiring its
    /// incoming blocks.
    pub(crate) const fn as_phi_mut(&mut self) -> Option<&mut Phi> {
        match self {
            Self::Phi(phi) => Some(phi),
            Self::Error
            | Self::Literal(_)
            | Self::RefOf(_)
            | Self::RefToPointer(_)
            | Self::Load(_)
            | Self::Binary(_)
            | Self::Call(_)
            | Self::Perform(_)
            | Self::Handle(_)
            | Self::Tuple(_)
            | Self::Closure(_)
            | Self::StructInitialization(_) => None,
        }
    }

    /// Returns the expressions this expression takes as operands, including
    /// the incoming values of a phi, in unspecified order.
    pub fn operands(&self) -> impl Iterator<Item = IRExprID> + '_ {
        // One variant per shape of operand list, which avoids boxing the
        // iterator.
        enum Iter<A, B, C, D, E, F, G> {
            None(A),
            Single(G),
            Pair(B),
            Slice(C),
            Slices(D),
            Phi(E),
            Fields(F),
        }

        impl<A, B, C, D, E, F, G> Iterator for Iter<A, B, C, D, E, F, G>
        where
            A: Iterator<Item = IRExprID>,
            B: Iterator<Item = IRExprID>,
            C: Iterator<Item = IRExprID>,
            D: Iterator<Item = IRExprID>,
            E: Iterator<Item = IRExprID>,
            F: Iterator<Item = IRExprID>,
            G: Iterator<Item = IRExprID>,
        {
            type Item = IRExprID;

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    Self::None(iter) => iter.next(),
                    Self::Single(iter) => iter.next(),
                    Self::Pair(iter) => iter.next(),
                    Self::Slice(iter) => iter.next(),
                    Self::Slices(iter) => iter.next(),
                    Self::Phi(iter) => iter.next(),
                    Self::Fields(iter) => iter.next(),
                }
            }
        }

        match self {
            Self::Error | Self::Literal(_) | Self::RefOf(_) | Self::Load(_) => {
                Iter::None(std::iter::empty())
            }
            Self::Phi(phi) => Iter::Phi(phi.incoming().map(|(_, value)| value)),
            Self::Binary(binary) => Iter::Pair([binary.left(), binary.right()].into_iter()),
            Self::Call(call) => Iter::Slice(call.arguments().iter().copied()),
            Self::Perform(perform) => Iter::Slice(perform.arguments().iter().copied()),
            Self::Tuple(tuple) => Iter::Slice(tuple.elements().iter().copied()),
            Self::RefToPointer(coercion) => Iter::Single(std::iter::once(coercion.reference())),
            Self::Closure(closure) => Iter::Slice(closure.captures().iter().copied()),
            Self::Handle(handle) => {
                Iter::Slices(handle.captures().iter().chain(handle.handler_captures()).copied())
            }
            Self::StructInitialization(initialization) => {
                Iter::Fields(initialization.initializers().values().copied())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct IRExpr {
    kind: IRExprKind,
    span: RelativeSpan,
    ty: Interned<Ty>,
}

impl IRExpr {
    #[must_use]
    pub fn new(kind: impl Into<IRExprKind>, span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind: kind.into(), span, ty }
    }

    #[must_use]
    pub fn new_error(span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self::new(IRExprKind::Error, span, ty)
    }

    #[must_use]
    pub const fn kind(&self) -> &IRExprKind { &self.kind }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct IRExpressionMap {
    expressions: Arena<IRExpr>,
}

impl IRExpressionMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn get_expression(&self, id: IRExprID) -> &IRExpr { self.expressions.get(id).unwrap() }

    pub fn insert_expression(&mut self, expression: IRExpr) -> IRExprID {
        self.expressions.insert(expression)
    }

    /// Returns the phi the expression `id` is, if it is one.
    pub(crate) fn phi_mut(&mut self, id: IRExprID) -> Option<&mut Phi> {
        self.expressions.get_mut(id).unwrap().kind.as_phi_mut()
    }

    pub(crate) fn expressions(&self) -> impl ExactSizeIterator<Item = (IRExprID, &IRExpr)> {
        self.expressions.iter()
    }
}

impl VisitType for IRExpr {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(&self.ty, site);
        self.kind.visit_types(site, visitor);
    }
}

/// Visits the types the operation itself uses, such as the substitution of a
/// call, but not the type of the value it produces; see [`IRExpr::ty`].
impl VisitType for IRExprKind {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        match self {
            Self::Call(call) => call.visit_types(site, visitor),
            Self::Perform(perform) => perform.visit_types(site, visitor),
            Self::Handle(handle) => handle.visit_types(site, visitor),

            Self::Error
            | Self::Literal(_)
            | Self::RefOf(_)
            | Self::RefToPointer(_)
            | Self::Load(_)
            | Self::Phi(_)
            | Self::Binary(_)
            | Self::Tuple(_)
            | Self::StructInitialization(_)
            | Self::Closure(_) => {}
        }
    }
}

impl VisitType for IRExpressionMap {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, expression) in self.expressions() {
            expression.visit_types(site, visitor);
        }
    }
}

impl VisitTypeMut for IRExpr {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type_mut(&mut self.ty, site);

        match &mut self.kind {
            IRExprKind::Call(call) => call.visit_types_mut(site, visitor),
            IRExprKind::Perform(perform) => perform.visit_types_mut(site, visitor),
            IRExprKind::Handle(handle) => handle.visit_types_mut(site, visitor),

            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::RefToPointer(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::StructInitialization(_)
            | IRExprKind::Closure(_) => {}
        }
    }
}

impl VisitTypeMutAsync for IRExpr {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        visitor.visit_type_mut_async(&mut self.ty, site).await;

        match &mut self.kind {
            IRExprKind::Call(call) => call.visit_types_mut_async(site, visitor).await,
            IRExprKind::Perform(perform) => perform.visit_types_mut_async(site, visitor).await,
            IRExprKind::Handle(handle) => handle.visit_types_mut_async(site, visitor).await,

            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::RefToPointer(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::StructInitialization(_)
            | IRExprKind::Closure(_) => {}
        }
    }
}

impl VisitTypeMut for IRExpressionMap {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        for (_, expression) in self.expressions.iter_mut() {
            expression.visit_types_mut(site, visitor);
        }
    }
}

impl VisitTypeMutAsync for IRExpressionMap {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        for (_, expression) in self.expressions.iter_mut() {
            expression.visit_types_mut_async(site, visitor).await;
        }
    }
}
