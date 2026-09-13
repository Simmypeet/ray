use qbice::{Decode, Encode, StableHash};

use crate::{ir_expr::IRExprID, ir_function::FunctionID};

/// Creates a nominal closure whose capture storage is described by its type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Closure {
    function_id: FunctionID,
    captures: Vec<IRExprID>,
}

impl Closure {
    #[must_use]
    pub const fn new(function_id: FunctionID, captures: Vec<IRExprID>) -> Self {
        Self { function_id, captures }
    }

    #[must_use]
    pub const fn function_id(&self) -> FunctionID { self.function_id }

    #[must_use]
    pub fn captures(&self) -> &[IRExprID] { &self.captures }
}
