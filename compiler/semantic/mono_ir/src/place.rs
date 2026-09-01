use qbice::{Decode, Encode, Identifiable, StableHash};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Projection {
    Dereference,
    EnvironmentFieldIndex(FieldIndex),
    TupleFieldIndex(FieldIndex),
    ClosureEnvironmentField,
    ClosureFunctionPointerField,
    OperationRecordEnvironmentField(FieldIndex),
    OperationRecordFunctionPointerField(FieldIndex),
}

/// An addressable `MonoIR` location.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct Place {
    local: LocalID,
    projections: Vec<Projection>,
}

impl Place {
    #[must_use]
    pub const fn new(local: LocalID) -> Self { Self { local, projections: Vec::new() } }

    #[must_use]
    pub fn with_projection(mut self, projection: Projection) -> Self {
        self.projections.push(projection);
        self
    }

    #[must_use]
    pub fn dereference(self) -> Self { self.with_projection(Projection::Dereference) }

    #[must_use]
    pub fn project_environment_field(self, field: FieldIndex) -> Self {
        self.with_projection(Projection::EnvironmentFieldIndex(field))
    }

    #[must_use]
    pub fn project_tuple_field(self, field: FieldIndex) -> Self {
        self.with_projection(Projection::TupleFieldIndex(field))
    }

    #[must_use]
    pub fn project_closure_environment(self) -> Self {
        self.with_projection(Projection::ClosureEnvironmentField)
    }

    #[must_use]
    pub fn project_closure_function_pointer(self) -> Self {
        self.with_projection(Projection::ClosureFunctionPointerField)
    }

    #[must_use]
    pub fn project_operation_record_environment(self, operation: FieldIndex) -> Self {
        self.with_projection(Projection::OperationRecordEnvironmentField(operation))
    }

    #[must_use]
    pub fn project_operation_record_function_pointer(self, operation: FieldIndex) -> Self {
        self.with_projection(Projection::OperationRecordFunctionPointerField(operation))
    }

    #[must_use]
    pub const fn local(&self) -> LocalID { self.local }

    #[must_use]
    pub fn projections(&self) -> &[Projection] { &self.projections }
}
