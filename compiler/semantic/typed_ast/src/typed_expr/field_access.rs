use qbice::{Decode, Encode, StableHash};
use rayc_semantic_element::struct_body::FieldID;

use crate::typed_expr::{SubExprs, TypedExprID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct FieldAccess {
    operand: TypedExprID,
    field: FieldID,
}

impl FieldAccess {
    #[must_use]
    pub const fn new(operand: TypedExprID, field: FieldID) -> Self { Self { operand, field } }

    #[must_use]
    pub const fn operand(&self) -> TypedExprID { self.operand }

    #[must_use]
    pub const fn field(&self) -> FieldID { self.field }
}

impl SubExprs for FieldAccess {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.operand) }
}
