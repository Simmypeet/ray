use qbice::{Decode, Encode, StableHash};

use crate::{
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionID,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Lambda {
    function_id: TypedFunctionID,
}

impl Lambda {
    #[must_use]
    pub const fn new(function_id: TypedFunctionID) -> Self { Self { function_id } }

    #[must_use]
    pub const fn function_id(&self) -> TypedFunctionID { self.function_id }
}

impl SubExprs for Lambda {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}
