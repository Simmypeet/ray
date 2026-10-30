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

    /// The field at a position counted in `usize`, as an iterator gives it.
    ///
    /// # Panics
    ///
    /// Panics if `index` does not fit in `u32`.
    #[must_use]
    pub fn from_usize(index: usize) -> Self {
        Self(index.try_into().expect("a field index should fit in u32"))
    }

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

    /// Starts a projection of this place, whose result `engine` interns.
    #[must_use]
    pub const fn project<'a>(&'a self, engine: &'a TrackedEngine) -> PlaceProjector<'a> {
        PlaceProjector { place: self, engine }
    }

    #[must_use]
    pub const fn local(&self) -> LocalID { self.local }

    #[must_use]
    pub fn projections(&self) -> &[Projection] { &self.projections }
}

/// A place about to be projected: each method gives the place of a part of it.
#[derive(Debug, Clone, Copy)]
pub struct PlaceProjector<'a> {
    place: &'a Place,
    engine: &'a TrackedEngine,
}

impl PlaceProjector<'_> {
    #[must_use]
    pub fn with_projection(self, projection: Projection) -> Place {
        let mut projections = self.place.projections.to_vec();
        projections.push(projection);
        Place { local: self.place.local, projections: self.engine.intern_unsized(projections) }
    }

    #[must_use]
    pub fn dereference(self) -> Place { self.with_projection(Projection::Dereference) }

    #[must_use]
    pub fn environment_field(self, field: FieldIndex) -> Place {
        self.with_projection(Projection::EnvironmentFieldIndex(field))
    }

    #[must_use]
    pub fn tuple_field(self, field: FieldIndex) -> Place {
        self.with_projection(Projection::TupleFieldIndex(field))
    }

    #[must_use]
    pub fn operation_record_environment(self, operation: GlobalSymbolID) -> Place {
        self.with_projection(Projection::OperationRecordEnvironmentField(operation))
    }

    #[must_use]
    pub fn operation_record_function_pointer(self, operation: GlobalSymbolID) -> Place {
        self.with_projection(Projection::OperationRecordFunctionPointerField(operation))
    }

    #[must_use]
    pub fn struct_field(self, field: FieldID) -> Place {
        self.with_projection(Projection::StructFieldIndex(field))
    }

    #[must_use]
    pub fn coroutine_frame_state(self) -> Place {
        self.with_projection(Projection::CoroutineFrameState)
    }

    #[must_use]
    pub fn coroutine_resume_value(self, value: FieldIndex) -> Place {
        self.with_projection(Projection::CoroutineResumeValue(value))
    }

    #[must_use]
    pub fn coroutine_progress_tag(self) -> Place {
        self.with_projection(Projection::CoroutineProgressTag)
    }

    #[must_use]
    pub fn coroutine_progress_yield(self) -> Place {
        self.with_projection(Projection::CoroutineProgressYield)
    }

    #[must_use]
    pub fn coroutine_progress_return(self) -> Place {
        self.with_projection(Projection::CoroutineProgressReturn)
    }

    #[must_use]
    pub fn coroutine_yield_tag(self) -> Place {
        self.with_projection(Projection::CoroutineYieldTag)
    }

    #[must_use]
    pub fn coroutine_yield_operation(self, operation: FieldIndex) -> Place {
        self.with_projection(Projection::CoroutineYieldOperation(operation))
    }
}
