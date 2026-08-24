use qbice::{Decode, Encode, StableHash};

use crate::{expression::ExpressionID, function::FunctionID};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct MakeLambda {
    function_id: FunctionID,
    captures: Vec<ExpressionID>,
}

impl MakeLambda {
    #[must_use]
    pub const fn new(function_id: FunctionID, captures: Vec<ExpressionID>) -> Self {
        Self { function_id, captures }
    }

    #[must_use]
    pub const fn function_id(&self) -> FunctionID { self.function_id }

    #[must_use]
    pub fn captures(&self) -> &[ExpressionID] { &self.captures }
}
