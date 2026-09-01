use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;

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
    OpaquePointer(PointerMutability),
    Pointer(PointerType),
    Aggregate(AggregateType),
    /// The nominal record type for one concrete effect instantiation.
    ///
    /// Its fields are described by the corresponding [`HandlerLayout`] in
    /// [`crate::MonoIR::handler_layouts`]. C code generation can emit that
    /// layout as a struct of callback closures and pass a pointer to the struct
    /// as a hidden parameter to effectful functions. The type is nominal so a
    /// callback that itself uses effects does not create a recursively expanded
    /// structural type.
    EffectHandler(MonoEffectInstance),
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
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum AggregateKind {
    Tuple,
    Closure,
    CaptureEnvironment,
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
    fields: Vec<Interned<MonoType>>,
}

impl AggregateType {
    #[must_use]
    pub const fn new(kind: AggregateKind, fields: Vec<Interned<MonoType>>) -> Self {
        Self { kind, fields }
    }

    #[must_use]
    pub const fn kind(&self) -> &AggregateKind { &self.kind }

    #[must_use]
    pub fn fields(&self) -> &[Interned<MonoType>] { &self.fields }
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
    variadic: bool,
}

impl FunctionSignature {
    #[must_use]
    pub const fn new(
        parameter_types: Interned<[Interned<MonoType>]>,
        return_type: ReturnType,
    ) -> Self {
        Self { parameter_types, return_type, variadic: false }
    }

    #[must_use]
    pub const fn new_variadic(
        parameter_types: Interned<[Interned<MonoType>]>,
        return_type: ReturnType,
    ) -> Self {
        Self { parameter_types, return_type, variadic: true }
    }

    #[must_use]
    pub fn parameter_types(&self) -> &[Interned<MonoType>] { &self.parameter_types }

    #[must_use]
    pub const fn return_type(&self) -> &ReturnType { &self.return_type }

    #[must_use]
    pub const fn is_variadic(&self) -> bool { self.variadic }
}

/// One callback slot in a concrete effect-handler record.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct EffectOperation {
    operation_id: GlobalSymbolID,
    signature: FunctionSignature,
}

impl EffectOperation {
    #[must_use]
    pub const fn new(operation_id: GlobalSymbolID, signature: FunctionSignature) -> Self {
        Self { operation_id, signature }
    }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub const fn signature(&self) -> &FunctionSignature { &self.signature }
}

/// The operation slots of one nominal, concrete effect-handler record.
///
/// C code generation can represent this as a struct with one closure field per
/// operation. Each closure consists of a callback address with the operation's
/// [`FunctionSignature`] and an opaque environment pointer. There is no
/// continuation or resumption slot: invoking an operation is an ordinary
/// callback call that returns to its caller.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct HandlerLayout {
    instance: MonoEffectInstance,
    operations: Vec<EffectOperation>,
}

impl HandlerLayout {
    #[must_use]
    pub const fn new(instance: MonoEffectInstance, operations: Vec<EffectOperation>) -> Self {
        Self { instance, operations }
    }

    #[must_use]
    pub const fn instance(&self) -> &MonoEffectInstance { &self.instance }

    #[must_use]
    pub fn operations(&self) -> &[EffectOperation] { &self.operations }
}
