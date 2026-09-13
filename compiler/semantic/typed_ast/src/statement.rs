use bon::Builder;
use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::RelativeSpan;

use crate::{
    name_binding::NameBindingGroupID, typed_expr::TypedExprID, typed_variable::TypedVariableID,
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct While {
    condition: TypedExprID,
    body: Vec<Statement>,
}

impl While {
    #[must_use]
    pub const fn new(condition: TypedExprID, body: Vec<Statement>) -> Self {
        Self { condition, body }
    }

    #[must_use]
    pub const fn condition(&self) -> TypedExprID { self.condition }

    pub fn body(&self) -> impl Iterator<Item = &Statement> { self.body.iter() }
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Statement {
    Let(Let),
    While(While),
    Break(Break),
    Continue(Continue),
    Expression(TypedExprID),
    Return(Return),
}
