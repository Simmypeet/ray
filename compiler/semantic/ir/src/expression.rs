use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::expression::{
    binary::Binary, call::Call, literal::Literal, load::Load, ref_of::RefOf, tuple::Tuple,
};

pub mod binary;
pub mod call;
pub mod literal;
pub mod load;
pub mod ref_of;
pub mod tuple;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum ExpressionKind {
    Error,
    Literal(Literal),
    RefOf(RefOf),
    Load(Load),
    Binary(Binary),
    Call(Call),
    Tuple(Tuple),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Expression {
    kind: ExpressionKind,
    span: Option<RelativeSpan>,
    ty: Interned<Ty>,
}

impl Expression {
    #[must_use]
    pub const fn new(kind: ExpressionKind, span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind, span: Some(span), ty }
    }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span.expect("Expression span should be set") }

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
    pub fn get_expression(&self, id: ID<Expression>) -> &Expression {
        self.expressions.get(id).unwrap()
    }

    pub fn insert_expression(&mut self, expression: Expression) -> ID<Expression> {
        self.expressions.insert(expression)
    }
}
