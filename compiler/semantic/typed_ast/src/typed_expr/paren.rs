use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::{SubExprs, TypedExprID};

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

impl SubExprs for Paren {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::once(self.expr) }
}
