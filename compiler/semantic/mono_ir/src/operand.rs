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
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Isize(i64),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Usize(u64),
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

/// A function address together with its concrete calling signature.
///
/// The signature is intentionally carried with the address even though local
/// functions also store it. In particular, a global [`FunctionReference`] only
/// identifies another `(definition, substitution)` fragment. Embedding the
/// signature lets this caller fragment type a function pointer and emit a C
/// declaration without loading the referenced function's `MonoIR` body. It also
/// keeps incremental dependencies on the callee's interface rather than its
/// implementation.
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
