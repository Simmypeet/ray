use qbice::{Decode, Encode, StableHash};

use crate::typed_function::FunctionID;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Lambda {
    function_id: FunctionID,
}

impl Lambda {
    #[must_use]
    pub const fn new(function_id: FunctionID) -> Self { Self { function_id } }

    #[must_use]
    pub const fn function_id(&self) -> FunctionID { self.function_id }
}
