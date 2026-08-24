use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Ty, TyApplicationView},
};

use crate::{
    name_binding::NameBindingID,
    typed_expr::{
        binary::Binary, call::Call, deref::Deref, errored::Errored, identifier::Identifier,
        if_else::IfElse, literal::Literal, paren::Paren, ref_of::RefOf, tuple::Tuple,
        tuple_index::TupleIndex,
    },
};

pub mod binary;
pub mod call;
pub mod deref;
pub mod errored;
pub mod identifier;
pub mod if_else;
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
    IfElse(IfElse),
    RefOf(RefOf),
    Deref(Deref),
    Paren(Paren),
    Errored(Errored),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvalueRoot {
    NameBinding(NameBindingID),
    Dereference(TypedExprID),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LvalueClassification {
    Lvalue(LvalueRoot),
    NotLvalue,
    Errored,
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

    #[must_use]
    pub const fn kind(&self) -> &TypedExprKind { &self.kind }
}

impl MutSubstitutable for TypedExpr {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);

        if let TypedExprKind::Call(call) = &mut self.kind {
            call.apply_mut_subst(subst, engine);
        }
    }
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

    #[must_use]
    pub fn classify_lvalue(&self, id: TypedExprID) -> LvalueClassification {
        let expression = self.get_expression(id);

        if matches!(
            &*expression.ty,
            Ty::Application(application) if application.view() == TyApplicationView::Error
        ) {
            return LvalueClassification::Errored;
        }

        match &expression.kind {
            TypedExprKind::Identifier(identifier) => {
                LvalueClassification::Lvalue(LvalueRoot::NameBinding(identifier.name_binding()))
            }
            TypedExprKind::TupleIndex(tuple_index) => self.classify_lvalue(tuple_index.operand()),
            TypedExprKind::Deref(deref) => {
                LvalueClassification::Lvalue(LvalueRoot::Dereference(deref.pointee()))
            }
            TypedExprKind::Paren(paren) => self.classify_lvalue(paren.expression()),
            TypedExprKind::Errored(_) => LvalueClassification::Errored,
            TypedExprKind::Literal(_)
            | TypedExprKind::Tuple(_)
            | TypedExprKind::Call(_)
            | TypedExprKind::Binary(_)
            | TypedExprKind::IfElse(_)
            | TypedExprKind::RefOf(_) => LvalueClassification::NotLvalue,
        }
    }
}

impl MutSubstitutable for TypedExprMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for expression in self.typed_exprs.items_mut() {
            expression.apply_mut_subst(subst, engine);
        }
    }
}
