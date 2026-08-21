use bon::Builder;
use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::RelativeSpan;

use crate::{name_binding::NameBindingGroupID, typed_expr::TypedExprID, variable::VariableID};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Builder,
)]
pub struct Let {
    variable_id: VariableID,
    name_binding_group_id: NameBindingGroupID,
    expression: TypedExprID,
    span: RelativeSpan,
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
pub enum Statement {
    Let(Let),
    Expression(TypedExprID),
    Return(Return),
}
