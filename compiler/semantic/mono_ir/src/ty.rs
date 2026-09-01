use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};

use crate::instance::MonoEffectInstance;

/// A fully concrete type with a direct runtime representation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum MonoType {
    Unit,
    Bool,
    Int32,
    Float32,
    CInt,
    CStr,
    Pointer(PointerType),
    Aggregate(AggregateType),
    FunctionPointer(FunctionSignature),
}

/// Whether writes through a pointer are permitted by the `MonoIR` type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum PointerMutability {
    Const,
    Mut,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct PointerType {
    pointee: Interned<MonoType>,
    mutability: PointerMutability,
}

impl PointerType {
    #[must_use]
    pub const fn new(pointee: Interned<MonoType>, mutability: PointerMutability) -> Self {
        Self { pointee, mutability }
    }

    #[must_use]
    pub const fn pointee(&self) -> &Interned<MonoType> { &self.pointee }

    #[must_use]
    pub const fn mutability(&self) -> PointerMutability { self.mutability }
}

/// Describes why an aggregate exists without assigning target-specific names
/// or layout rules to it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum AggregateKind {
    Tuple,
    Closure,
    CaptureEnvironment,
    EffectHandler(MonoEffectInstance),
}

/// A structural aggregate type.
///
/// Structural types keep independently cached `MonoIR` fragments composable:
/// the future orchestrator can deduplicate equal layouts without remapping
/// fragment-local type identifiers.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct AggregateType {
    kind: AggregateKind,
    fields: Vec<MonoType>,
}

impl AggregateType {
    #[must_use]
    pub const fn new(kind: AggregateKind, fields: Vec<MonoType>) -> Self { Self { kind, fields } }

    #[must_use]
    pub const fn kind(&self) -> &AggregateKind { &self.kind }

    #[must_use]
    pub fn fields(&self) -> &[MonoType] { &self.fields }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum ReturnType {
    Void,
    Value(Interned<[Interned<MonoType>]>),
}

/// A concrete calling signature shared by direct and indirect calls.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct FunctionSignature {
    parameter_types: Interned<[Interned<MonoType>]>,
    return_type: ReturnType,
}

impl FunctionSignature {
    #[must_use]
    pub fn parameter_types(&self) -> &[Interned<MonoType>] { &self.parameter_types }
}
