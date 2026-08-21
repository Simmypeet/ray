use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;

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

    #[must_use]
    pub const fn function_id(&self) -> GlobalSymbolID { self.function_id }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExprID] { &self.arguments }
}
