use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    ir_expr::{
        binary::Binary, call::Call, handle::Handle, literal::Literal, load::Load,
        make_lambda::MakeLambda, perform::Perform, phi::Phi, ref_of::RefOf, tuple::Tuple,
    },
    visit::{TypeVisitor, VisitType},
};

pub mod binary;
pub mod call;
pub mod handle;
pub mod literal;
pub mod load;
pub mod make_lambda;
pub mod perform;
pub mod phi;
pub mod ref_of;
pub mod tuple;

/// Identifies an expression value stored in a function's expression arena.
pub type IRExprID = ID<IRExpr>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum IRExprKind {
    Error,
    Literal(Literal),
    RefOf(RefOf),
    Load(Load),
    Phi(Phi),
    Binary(Binary),
    Call(Call),
    Perform(Perform),
    Handle(Handle),
    Tuple(Tuple),
    MakeLambda(MakeLambda),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct IRExpr {
    kind: IRExprKind,
    span: RelativeSpan,
    ty: Interned<Ty>,
}

impl IRExpr {
    #[must_use]
    pub const fn new(kind: IRExprKind, span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind, span, ty }
    }

    #[must_use]
    pub const fn new_error(span: RelativeSpan, ty: Interned<Ty>) -> Self {
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

    pub(crate) fn expressions(&self) -> impl ExactSizeIterator<Item = (IRExprID, &IRExpr)> {
        self.expressions.iter()
    }
}

impl VisitType for IRExpr {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        visitor.visit_type(&self.ty);

        match &self.kind {
            IRExprKind::Call(call) => call.visit_types(visitor),
            IRExprKind::Perform(perform) => perform.visit_types(visitor),
            IRExprKind::Handle(handle) => handle.visit_types(visitor),

            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::MakeLambda(_) => {}
        }
    }
}

impl VisitType for IRExpressionMap {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, expression) in self.expressions() {
            expression.visit_types(visitor);
        }
    }
}
