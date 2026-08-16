use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Paren {
    expr: TypedExprID,
}

impl Paren {
    #[must_use]
    pub const fn new(expr: TypedExprID) -> Self { Self { expr } }

    #[must_use]
    pub const fn expression(&self) -> TypedExprID { self.expr }
}
