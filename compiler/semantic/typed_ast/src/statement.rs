use bon::Builder;
use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

use crate::{
    name_binding::NameBindingGroupID,
    typed_expr::{SubExprs, TypedExprID},
    typed_variable::TypedVariableID,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct Let {
    variable_id: TypedVariableID,
    name_binding_group_id: NameBindingGroupID,
    expression: Option<TypedExprID>,
    span: RelativeSpan,
}

impl Let {
    #[must_use]
    pub const fn variable_id(&self) -> TypedVariableID { self.variable_id }

    #[must_use]
    pub const fn expression(&self) -> Option<TypedExprID> { self.expression }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Return {
    value: Option<TypedExprID>,
}

impl Return {
    #[must_use]
    pub const fn new_unit() -> Self { Self { value: None } }

    #[must_use]
    pub const fn new_with_value(value: TypedExprID) -> Self { Self { value: Some(value) } }

    #[must_use]
    pub const fn value(&self) -> Option<TypedExprID> { self.value }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Break {
    span: RelativeSpan,
}

impl Break {
    #[must_use]
    pub const fn new(span: RelativeSpan) -> Self { Self { span } }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Continue {
    span: RelativeSpan,
}

impl Continue {
    #[must_use]
    pub const fn new(span: RelativeSpan) -> Self { Self { span } }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// Evaluates an expression and drops its unused value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct ExpressionStatement {
    expression: TypedExprID,

    /// The `Drop` dictionary selected for the expression's type.
    drop_instance: Interned<Ty>,
}

impl ExpressionStatement {
    #[must_use]
    pub const fn new(expression: TypedExprID, drop_instance: Interned<Ty>) -> Self {
        Self { expression, drop_instance }
    }

    #[must_use]
    pub const fn expression(&self) -> TypedExprID { self.expression }

    #[must_use]
    pub const fn drop_instance(&self) -> &Interned<Ty> { &self.drop_instance }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Statement {
    Let(Let),
    Break(Break),
    Continue(Continue),
    Expression(ExpressionStatement),
    Return(Return),
}

impl MutSubstitutable for Statement {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        match self {
            Self::Expression(statement) => statement.drop_instance.apply_in_place(subst, engine),
            Self::Let(_) | Self::Break(_) | Self::Continue(_) | Self::Return(_) => {}
        }
    }
}

impl SubExprs for Statement {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        match self {
            Self::Let(statement) => statement.expression(),
            Self::Expression(statement) => Some(statement.expression()),
            Self::Return(statement) => statement.value(),
            Self::Break(_) | Self::Continue(_) => None,
        }
        .into_iter()
    }
}
