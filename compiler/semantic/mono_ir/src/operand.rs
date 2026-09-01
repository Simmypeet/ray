use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};

use crate::{
    instance::FunctionReference,
    place::Place,
    ty::{FunctionSignature, MonoType},
};

/// A scalar or aggregate constant whose type is inherent in its variant.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Constant {
    Unit,
    Bool(bool),
    Int32(i32),
    /// Stores the IEEE-754 bit pattern so constants retain total equality.
    Float32(u32),
    CInt(i32),
    CStr(Interned<str>),
    NullPointer(Interned<MonoType>),
}

impl Constant {
    #[must_use]
    pub const fn new_float32(value: f32) -> Self { Self::Float32(value.to_bits()) }
}

/// A function address together with the concrete signature needed to call it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct FunctionOperand {
    function: FunctionReference,
    signature: FunctionSignature,
}

impl FunctionOperand {
    #[must_use]
    pub const fn new(function: FunctionReference, signature: FunctionSignature) -> Self {
        Self { function, signature }
    }

    #[must_use]
    pub const fn function(&self) -> &FunctionReference { &self.function }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }
}

/// An atomic input to a `MonoIR` operation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Operand {
    Copy(Place),
    Constant(Constant),
    Function(FunctionOperand),
}
