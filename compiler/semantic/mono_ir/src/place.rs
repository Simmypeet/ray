use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_coroutine_frame_layout::savable::Savable;
use rayc_ir::ir_closure::CaptureID;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::struct_body::FieldID;
use rayc_symbol::GlobalSymbolID;

use crate::{
    function::LocalID,
    layout::{
        HandlerRecordID,
        coroutine::{ResumeVariantID, RowOperationID},
    },
};

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

/// Field access to the coroutine progress type. See
/// [`crate::ty::CoroutineProgress`] for the fields of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum CoroutineProgress {
    /// Roughtly corresponds to: `coroutine_progress_t.tag` in C.
    Tag,
    /// Rougly, corresponds to:  `coroutine_progress_t.payload.ret` in C.
    ReturnPayload,
    /// Roughly corresponds to: `coroutine_progress_t.payload.yield` in C.
    ///
    /// It has the type of the `coroutine_yield_t`. See
    /// [`crate::ty::CoroutineYield`] further for the fields of the type.
    YieldPayload,
}

/// Field access to the coroutine yield type. See [`crate::ty::CoroutineYield`]
/// for the fields of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum CoroutineYield {
    /// Roughtly corresponds to: `coroutine_yield_t.tag` in C.
    Tag,
    /// Roughly corresponds to: `coroutine_yield_t.payload.<operation>` in C.
    OperationPayload(RowOperationID),
}

/// Field access to a coroutine frame. See
/// [`crate::ty::AggregateType::CoroutineFrame`] for the fields of the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum CoroutineFrame {
    /// Roughly corresponds to: `coroutine_frame_t.tag` in C.
    Tag,

    /// The member of `coroutine_frame_t` that holds the value.
    ///
    /// [`FrameLayout`] determines which part of the frame holds it, but the
    /// member is named after the value alone, wherever it is placed.
    ///
    /// [`FrameLayout`]: rayc_coroutine_frame_layout::frame_layout::FrameLayout
    Savable(Savable),
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
    /// A capture of an environment, named by the capture it holds.
    EnvironmentCapture(CaptureID),
    /// A handler record stored in an environment, which only an operation
    /// handler's environment has.
    EnvironmentHandlerRecord(HandlerRecordID),
    TupleFieldIndex(FieldIndex),
    StructFieldIndex(FieldID),
    OperationRecordEnvironmentField(GlobalSymbolID),
    OperationRecordFunctionPointerField(GlobalSymbolID),
    CoroutineFrame(CoroutineFrame),
    /// Access to the variant of [`CoroutineResume`]
    ///
    /// [`CoroutineResume`]: crate::ty::CoroutineResume
    CoroutineResumeValue(ResumeVariantID),
    CoroutineProgress(CoroutineProgress),
    CoroutineYield(CoroutineYield),
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
    pub fn environment_capture(self, capture: CaptureID) -> Place {
        self.with_projection(Projection::EnvironmentCapture(capture))
    }

    #[must_use]
    pub fn environment_handler_record(self, handler_record: HandlerRecordID) -> Place {
        self.with_projection(Projection::EnvironmentHandlerRecord(handler_record))
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
        self.with_projection(Projection::CoroutineFrame(CoroutineFrame::Tag))
    }

    #[must_use]
    pub fn coroutine_frame_savable(self, savable: Savable) -> Place {
        self.with_projection(Projection::CoroutineFrame(CoroutineFrame::Savable(savable)))
    }

    #[must_use]
    pub fn coroutine_resume_value(self, resume_variant: ResumeVariantID) -> Place {
        self.with_projection(Projection::CoroutineResumeValue(resume_variant))
    }

    #[must_use]
    pub fn coroutine_progress_tag(self) -> Place {
        self.with_projection(Projection::CoroutineProgress(CoroutineProgress::Tag))
    }

    #[must_use]
    pub fn coroutine_progress_yield_payload(self) -> Place {
        self.with_projection(Projection::CoroutineProgress(CoroutineProgress::YieldPayload))
    }

    #[must_use]
    pub fn coroutine_progress_return_payload(self) -> Place {
        self.with_projection(Projection::CoroutineProgress(CoroutineProgress::ReturnPayload))
    }

    #[must_use]
    pub fn coroutine_yield_tag(self) -> Place {
        self.with_projection(Projection::CoroutineYield(CoroutineYield::Tag))
    }

    #[must_use]
    pub fn coroutine_yield_operation(self, operation: RowOperationID) -> Place {
        self.with_projection(Projection::CoroutineYield(CoroutineYield::OperationPayload(
            operation,
        )))
    }
}
