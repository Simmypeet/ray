use qbice::{Decode, Encode, StableHash};

use crate::{
    statement::Statement,
    typed_expr::{SubExprs, TypedExprID},
};

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

impl SubExprs for While {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> {
        std::iter::once(self.condition).chain(self.body.iter().flat_map(SubExprs::sub_exprs))
    }
}
