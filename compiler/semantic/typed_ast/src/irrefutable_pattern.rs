use qbice::{Decode, Encode, StableHash};

use crate::typed_variable::TypedVariableID;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Hash, StableHash, Encode, Decode)]
pub enum IrrefutablePattern {
    Name(TypedVariableID),
    Error,
}
