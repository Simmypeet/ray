use qbice::{Decode, Encode, StableHash};

use crate::{ir_closure::CaptureOperands, ir_function::IRFunctionID};

/// Creates a nominal closure whose capture storage is described by its type.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct Closure {
    function_id: IRFunctionID,
    /// The operand stored in each capture of the closure body's capture map.
    captures: CaptureOperands,
}

impl Closure {
    #[must_use]
    pub const fn new(function_id: IRFunctionID, captures: CaptureOperands) -> Self {
        Self { function_id, captures }
    }

    #[must_use]
    pub const fn function_id(&self) -> IRFunctionID { self.function_id }

    /// The operand stored in each capture of the closure body's capture map.
    #[must_use]
    pub const fn captures(&self) -> &CaptureOperands { &self.captures }
}
