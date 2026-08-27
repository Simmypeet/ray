use qbice::{Decode, Encode, StableHash};

use crate::{
    name_binding::NameBindingID,
    typed_expr::{SubExprs, TypedExprID},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Identifier {
    name_binding: NameBindingID,
}

impl Identifier {
    #[must_use]
    pub const fn new(name_binding: NameBindingID) -> Self { Self { name_binding } }

    #[must_use]
    pub const fn name_binding(&self) -> NameBindingID { self.name_binding }
}

impl SubExprs for Identifier {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}
