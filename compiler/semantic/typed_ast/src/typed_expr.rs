use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};

use crate::typed_expr::{
    binary::Binary, call::Call, deref::Deref, errored::Errored, identifier::Identifier,
    literal::Literal, paren::Paren, ref_of::RefOf, r#return::Return, tuple::Tuple,
    tuple_index::TupleIndex,
};

pub mod binary;
pub mod call;
pub mod deref;
pub mod errored;
pub mod identifier;
pub mod literal;
pub mod paren;
pub mod ref_of;
pub mod r#return;
pub mod tuple;
pub mod tuple_index;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub enum TypedExprKind {
    Identifier(Identifier),
    Literal(Literal),
    TupleIndex(TupleIndex),
    Tuple(Tuple),
    Call(Call),
    Binary(Binary),
    RefOf(RefOf),
    Deref(Deref),
    Paren(Paren),
    Return(Return),
    Errored(Errored),
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct TypedExpr {
    kind: TypedExprKind,
    span: RelativeSpan,
    ty: Interned<Ty>,
}

impl TypedExpr {
    #[must_use]
    pub const fn new(kind: TypedExprKind, span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind, span, ty }
    }

    #[must_use]
    pub const fn new_error(span: RelativeSpan, ty: Interned<Ty>) -> Self {
        Self { kind: TypedExprKind::Errored(Errored::new_empty()), span, ty }
    }

    #[must_use]
    pub const fn new_error_with_children(
        children: Vec<TypedExprID>,
        span: RelativeSpan,
        ty: Interned<Ty>,
    ) -> Self {
        Self { kind: TypedExprKind::Errored(Errored::new(children)), span, ty }
    }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

pub type TypedExprID = ID<TypedExpr>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct TypedExprMap {
    typed_exprs: Arena<TypedExpr>,
}

impl TypedExprMap {
    #[must_use]
    pub fn get_expression(&self, id: TypedExprID) -> &TypedExpr {
        self.typed_exprs.get(id).expect("TypedExprID should be valid")
    }

    #[must_use]
    pub fn insert_expression(&mut self, expression: TypedExpr) -> TypedExprID {
        self.typed_exprs.insert(expression)
    }
}
