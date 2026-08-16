use rayc_symbol::GlobalSymbolID;
use qbice::{Decode, Encode, StableHash};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Call {
    function_id: GlobalSymbolID,
    arguments: Vec<TypedExprID>,
}

impl Call {
    #[must_use]
    pub const fn new(function_id: GlobalSymbolID, arguments: Vec<TypedExprID>) -> Self {
        Self { function_id, arguments }
    }
}
