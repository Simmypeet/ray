use qbice::{Decode, Encode, StableHash};
use rayc_arena::ID;
use rayc_symbol::GlobalSymbolID;

use crate::expression::Expression;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Call {
    function_id: GlobalSymbolID,
    arguments: Vec<ID<Expression>>,
}

impl Call {
    #[must_use]
    pub const fn new(function_id: GlobalSymbolID, arguments: Vec<ID<Expression>>) -> Self {
        Self { function_id, arguments }
    }
}
