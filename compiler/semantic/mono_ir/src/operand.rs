use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};

use crate::{instance::FunctionReference, place::Place, ty::MonoType};

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

/// An atomic input to a `MonoIR` operation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum Operand {
    Copy(Place),
    Constant(Constant),
    /// The address of a function. Its signature is that of the referenced
    /// function: a local function's own, or
    /// [`get_def_signature`](crate::signature::get_def_signature) for a
    /// global definition. Callers thus depend on a callee's declaration
    /// rather than its lowered body.
    Function(FunctionReference),
}
