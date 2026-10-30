use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::struct_body::FieldID;
use rayc_symbol::GlobalSymbolID;

use crate::function::LocalID;

/// The zero-based field position within an aggregate value.
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
pub struct FieldIndex(u32);

impl FieldIndex {
    #[must_use]
    pub const fn new(index: u32) -> Self { Self(index) }

    #[must_use]
    pub const fn index(self) -> u32 { self.0 }
}

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
pub enum Projection {
    Dereference,
    EnvironmentFieldIndex(FieldIndex),
    TupleFieldIndex(FieldIndex),
    StructFieldIndex(FieldID),
    OperationRecordEnvironmentField(GlobalSymbolID),
    OperationRecordFunctionPointerField(GlobalSymbolID),
    /// The state of a coroutine frame: where it runs from when resumed.
    CoroutineFrameState,
    CoroutineFramePrefixField(FieldIndex),
    CoroutineFrameStartField(FieldIndex),
    CoroutineFrameVariantField {
        variant: FieldIndex,
        field: FieldIndex,
    },
    /// The member of a `coroutine_resume_t` of one of the row's resume types.
    CoroutineResumeValue(FieldIndex),
    /// Whether a `coroutine_progress_t` holds a yield or a return value.
    CoroutineProgressTag,
    CoroutineProgressYield,
    CoroutineProgressReturn,
    /// Which operation of the row a `coroutine_yield_t` performs.
    CoroutineYieldTag,
    /// The arguments of the operation of a `coroutine_yield_t`.
    CoroutineYieldOperation(FieldIndex),
}

/// An addressable `MonoIR` location.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Place {
    local: LocalID,
    projections: Interned<[Projection]>,
}

impl Place {
    #[must_use]
    pub fn new(local: LocalID, engine: &TrackedEngine) -> Self {
        Self { local, projections: engine.intern_unsized(Vec::new()) }
    }

    #[must_use]
    pub fn with_projection(self, projection: Projection, engine: &TrackedEngine) -> Self {
        let mut projections = self.projections.to_vec();
        projections.push(projection);
        Self { local: self.local, projections: engine.intern_unsized(projections) }
    }

    #[must_use]
    pub fn dereference(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::Dereference, engine)
    }

    #[must_use]
    pub fn project_environment_field(self, field: FieldIndex, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::EnvironmentFieldIndex(field), engine)
    }

    #[must_use]
    pub fn project_tuple_field(self, field: FieldIndex, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::TupleFieldIndex(field), engine)
    }

    #[must_use]
    pub fn project_operation_record_environment(
        self,
        operation: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Self {
        self.with_projection(Projection::OperationRecordEnvironmentField(operation), engine)
    }

    #[must_use]
    pub fn project_operation_record_function_pointer(
        self,
        operation: GlobalSymbolID,
        engine: &TrackedEngine,
    ) -> Self {
        self.with_projection(Projection::OperationRecordFunctionPointerField(operation), engine)
    }

    #[must_use]
    pub fn project_struct_field(self, field: FieldID, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::StructFieldIndex(field), engine)
    }

    #[must_use]
    pub fn project_coroutine_frame_state(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineFrameState, engine)
    }

    #[must_use]
    pub fn project_coroutine_resume_value(self, value: FieldIndex, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineResumeValue(value), engine)
    }

    #[must_use]
    pub fn project_coroutine_progress_tag(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineProgressTag, engine)
    }

    #[must_use]
    pub fn project_coroutine_progress_yield(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineProgressYield, engine)
    }

    #[must_use]
    pub fn project_coroutine_progress_return(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineProgressReturn, engine)
    }

    #[must_use]
    pub fn project_coroutine_yield_tag(self, engine: &TrackedEngine) -> Self {
        self.with_projection(Projection::CoroutineYieldTag, engine)
    }

    #[must_use]
    pub fn project_coroutine_yield_operation(
        self,
        operation: FieldIndex,
        engine: &TrackedEngine,
    ) -> Self {
        self.with_projection(Projection::CoroutineYieldOperation(operation), engine)
    }

    #[must_use]
    pub const fn local(&self) -> LocalID { self.local }

    #[must_use]
    pub fn projections(&self) -> &[Projection] { &self.projections }
}
