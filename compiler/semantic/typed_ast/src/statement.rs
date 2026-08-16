use bon::Builder;
use rayc_lexical::tree::RelativeSpan;
use qbice::{Decode, Encode, StableHash};

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
pub enum Statement {
    Let(Let),
    Expression(TypedExprID),
}
