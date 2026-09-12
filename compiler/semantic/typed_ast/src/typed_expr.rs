use derive_more::From;
use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView},
};

use crate::{
    name_binding::NameBindingID,
    typed_expr::{
        binary::Binary, call::Call, deref::Deref, errored::Errored, identifier::Identifier,
        if_else::IfElse, lambda::Lambda, literal::Literal, nlambda::NLambda, paren::Paren,
        ref_of::RefOf, run_with::RunWith, tuple::Tuple, tuple_index::TupleIndex,
    },
};

pub mod binary;
pub mod call;
pub mod deref;
pub mod errored;
pub mod identifier;
pub mod if_else;
pub mod lambda;
pub mod literal;
pub mod nlambda;
pub mod paren;
pub mod ref_of;
pub mod r#return;
pub mod run_with;
pub mod tuple;
pub mod tuple_index;

/// Retrieves the child expressions of the current expression node.
pub trait SubExprs {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID>;
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, From)]
pub enum TypedExprKind {
    Identifier(Identifier),
    Literal(Literal),
    TupleIndex(TupleIndex),
    Tuple(Tuple),
    Call(Call),
    Lambda(Lambda),
    NLambda(NLambda),
    Binary(Binary),
    IfElse(IfElse),
    RefOf(RefOf),
    Deref(Deref),
    Paren(Paren),
    RunWith(RunWith),
    Errored(Errored),
}

impl SubExprs for TypedExprKind {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        // There must be a better way to do this while doesn't require boxing the
        // iterator 😭
        pub enum Iter<A, B, C, D, E, F, G, H, I, J, K, L, M, N> {
            A(A),
            B(B),
            C(C),
            D(D),
            E(E),
            F(F),
            G(G),
            H(H),
            I(I),
            J(J),
            K(K),
            L(L),
            M(M),
            N(N),
        }

        impl<A, B, C, D, E, F, G, H, I, J, K, L, M, N> Iterator
            for Iter<A, B, C, D, E, F, G, H, I, J, K, L, M, N>
        where
            A: Iterator<Item = TypedExprID>,
            B: Iterator<Item = TypedExprID>,
            C: Iterator<Item = TypedExprID>,
            D: Iterator<Item = TypedExprID>,
            E: Iterator<Item = TypedExprID>,
            F: Iterator<Item = TypedExprID>,
            G: Iterator<Item = TypedExprID>,
            H: Iterator<Item = TypedExprID>,
            I: Iterator<Item = TypedExprID>,
            J: Iterator<Item = TypedExprID>,
            K: Iterator<Item = TypedExprID>,
            L: Iterator<Item = TypedExprID>,
            M: Iterator<Item = TypedExprID>,
            N: Iterator<Item = TypedExprID>,
        {
            type Item = TypedExprID;

            fn next(&mut self) -> Option<Self::Item> {
                match self {
                    Self::A(iter) => iter.next(),
                    Self::B(iter) => iter.next(),
                    Self::C(iter) => iter.next(),
                    Self::D(iter) => iter.next(),
                    Self::E(iter) => iter.next(),
                    Self::F(iter) => iter.next(),
                    Self::G(iter) => iter.next(),
                    Self::H(iter) => iter.next(),
                    Self::I(iter) => iter.next(),
                    Self::J(iter) => iter.next(),
                    Self::K(iter) => iter.next(),
                    Self::L(iter) => iter.next(),
                    Self::M(iter) => iter.next(),
                    Self::N(iter) => iter.next(),
                }
            }
        }

        match self {
            Self::Literal(x) => Iter::A(x.sub_exprs()),
            Self::Identifier(x) => Iter::B(x.sub_exprs()),
            Self::TupleIndex(x) => Iter::C(x.sub_exprs()),
            Self::Tuple(x) => Iter::D(x.sub_exprs()),
            Self::Call(x) => Iter::E(x.sub_exprs()),
            Self::Lambda(x) => Iter::F(x.sub_exprs()),
            Self::NLambda(x) => Iter::N(x.sub_exprs()),
            Self::Binary(x) => Iter::G(x.sub_exprs()),
            Self::IfElse(x) => Iter::H(x.sub_exprs()),
            Self::RefOf(x) => Iter::I(x.sub_exprs()),
            Self::Deref(x) => Iter::J(x.sub_exprs()),
            Self::Paren(x) => Iter::K(x.sub_exprs()),
            Self::RunWith(x) => Iter::L(x.sub_exprs()),
            Self::Errored(x) => Iter::M(x.sub_exprs()),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct TypedExpr {
    kind: TypedExprKind,
    span: RelativeSpan,
    ty: Interned<Ty>,
    effect: Interned<Ty>,
}

impl TypedExpr {
    #[must_use]
    pub const fn new(
        kind: TypedExprKind,
        span: RelativeSpan,
        ty: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> Self {
        Self { kind, span, ty, effect }
    }

    #[must_use]
    pub const fn new_error(span: RelativeSpan, ty: Interned<Ty>, effect: Interned<Ty>) -> Self {
        Self { kind: TypedExprKind::Errored(Errored::new_empty()), span, ty, effect }
    }

    #[must_use]
    pub const fn new_error_with_children(
        children: Vec<TypedExprID>,
        span: RelativeSpan,
        ty: Interned<Ty>,
        effect: Interned<Ty>,
    ) -> Self {
        Self { kind: TypedExprKind::Errored(Errored::new(children)), span, ty, effect }
    }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }

    #[must_use]
    pub const fn kind(&self) -> &TypedExprKind { &self.kind }
}

impl MutSubstitutable for TypedExpr {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);
        self.effect.apply_in_place(subst, engine);

        match &mut self.kind {
            TypedExprKind::Call(call) => call.apply_mut_subst(subst, engine),
            TypedExprKind::RunWith(run_with) => run_with.apply_mut_subst(subst, engine),
            TypedExprKind::Identifier(_)
            | TypedExprKind::Literal(_)
            | TypedExprKind::TupleIndex(_)
            | TypedExprKind::Tuple(_)
            | TypedExprKind::Lambda(_)
            | TypedExprKind::NLambda(_)
            | TypedExprKind::Binary(_)
            | TypedExprKind::IfElse(_)
            | TypedExprKind::RefOf(_)
            | TypedExprKind::Deref(_)
            | TypedExprKind::Paren(_)
            | TypedExprKind::Errored(_) => {}
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
            Ty::Application(application) if application.view() == ApplicationView::Error
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
            | TypedExprKind::Lambda(_)
            | TypedExprKind::NLambda(_)
            | TypedExprKind::Binary(_)
            | TypedExprKind::IfElse(_)
            | TypedExprKind::RefOf(_)
            | TypedExprKind::RunWith(_) => LvalueClassification::NotLvalue,
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
