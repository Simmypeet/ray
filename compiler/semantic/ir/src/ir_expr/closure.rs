use qbice::{Decode, Encode, StableHash};

use crate::{ir_expr::IRExprID, ir_function::IRFunctionID};

/// Creates a nominal closure whose capture storage is described by its type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Closure {
    function_id: IRFunctionID,
    captures: Vec<IRExprID>,
}

impl Closure {
    #[must_use]
    pub const fn new(function_id: IRFunctionID, captures: Vec<IRExprID>) -> Self {
        Self { function_id, captures }
    }

    #[must_use]
    pub const fn function_id(&self) -> IRFunctionID { self.function_id }

    #[must_use]
    pub fn captures(&self) -> &[IRExprID] { &self.captures }
}
