use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct IfElse {
    condition: TypedExprID,
    then_expression: TypedExprID,
    else_expression: TypedExprID,
}

impl IfElse {
    #[must_use]
    pub const fn new(
        condition: TypedExprID,
        then_expression: TypedExprID,
        else_expression: TypedExprID,
    ) -> Self {
        Self { condition, then_expression, else_expression }
    }

    #[must_use]
    pub const fn condition(&self) -> TypedExprID { self.condition }

    #[must_use]
    pub const fn then_expression(&self) -> TypedExprID { self.then_expression }

    #[must_use]
    pub const fn else_expression(&self) -> TypedExprID { self.else_expression }
}
