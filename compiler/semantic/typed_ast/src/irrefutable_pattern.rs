use qbice::{Decode, Encode, StableHash};

use crate::variable::VariableID;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, StableHash, Encode, Decode)]
pub enum IrrefutablePattern {
    Name(VariableID),
    Error,
}
