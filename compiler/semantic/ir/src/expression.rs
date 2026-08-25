use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    expression::{
        binary::Binary, call::Call, literal::Literal, load::Load, make_lambda::MakeLambda,
        phi::Phi, ref_of::RefOf, tuple::Tuple,
    },
    visit::{TypeVisitor, VisitType},
};

pub mod binary;
pub mod call;
pub mod literal;
pub mod load;
pub mod make_lambda;
pub mod phi;
pub mod ref_of;
pub mod tuple;

/// Identifies an expression value stored in a function's expression arena.
pub type ExpressionID = ID<Expression>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum ExpressionKind {
    Error,
    Literal(Literal),
    RefOf(RefOf),
    Load(Load),
    Phi(Phi),
    Binary(Binary),
    Call(Call),
    Tuple(Tuple),
    MakeLambda(MakeLambda),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Expression {
    kind: ExpressionKind,
    span: RelativeSpan,
    ty: Interned<Ty>,
}

impl Expression {
    #[must_use]
    pub const fn new(kind: ExpressionKind, span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind, span, ty }
    }

    #[must_use]
    pub const fn new_error(span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self::new(ExpressionKind::Error, span, ty)
    }

    #[must_use]
    pub const fn kind(&self) -> &ExpressionKind { &self.kind }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct ExpressionMap {
    expressions: Arena<Expression>,
}

impl ExpressionMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn get_expression(&self, id: ExpressionID) -> &Expression {
        self.expressions.get(id).unwrap()
    }

    pub fn insert_expression(&mut self, expression: Expression) -> ExpressionID {
        self.expressions.insert(expression)
    }

    pub(crate) fn expressions(&self) -> impl ExactSizeIterator<Item = (ExpressionID, &Expression)> {
        self.expressions.iter()
    }
}

impl VisitType for Expression {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        visitor.visit_type(&self.ty);

        match &self.kind {
            ExpressionKind::Call(call) => call.visit_types(visitor),

            ExpressionKind::Error
            | ExpressionKind::Literal(_)
            | ExpressionKind::RefOf(_)
            | ExpressionKind::Load(_)
            | ExpressionKind::Phi(_)
            | ExpressionKind::Binary(_)
            | ExpressionKind::Tuple(_)
            | ExpressionKind::MakeLambda(_) => {}
        }
    }
}

impl VisitType for ExpressionMap {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, expression) in self.expressions() {
            expression.visit_types(visitor);
        }
    }
}
