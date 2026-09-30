use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::{parameter::ParameterID, struct_body::FieldID};

use crate::{
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};

/// The storage an [`Address`] starts from.
///
/// Every root names storage owned by the current function's frame. Memory
/// reached through a pointer is expressed with a dereference
/// [`Projection`] instead, so the pointer's own place stays visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum AddressRoot {
    Error,
    Variable(IRVariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    OperationHandlerParameter(OperationHandlerParameterID),
    Capture(CaptureID),
}

/// A storage location owned by a function's frame: a local variable,
/// including temporaries, a parameter, or a capture of a nested function.
///
/// Every [`AddressRoot`] other than [`AddressRoot::Error`] names a local.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Local {
    Variable(IRVariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    OperationHandlerParameter(OperationHandlerParameterID),
    Capture(CaptureID),
}

impl Local {
    /// Returns the address of the whole local.
    #[must_use]
    pub fn to_address(self, engine: &TrackedEngine) -> Address {
        match self {
            Self::Variable(variable) => Address::new_variable(variable, engine),
            Self::Parameter(parameter) => Address::new_parameter(parameter, engine),
            Self::LambdaParameter(parameter) => Address::new_lambda_parameter(parameter, engine),
            Self::OperationHandlerParameter(parameter) => {
                Address::new_operation_handler_parameter(parameter, engine)
            }
            Self::Capture(capture) => Address::new_capture(capture, engine),
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Projection {
    Tuple(usize),
    Field(FieldID),

    /// Dereferences the reference stored at the address so far.
    ///
    /// The memory behind a reference is borrowed, not owned by the stack
    /// frame, so nothing may be moved out of it.
    Deref,

    /// Dereferences the raw pointer stored at the address so far.
    ///
    /// The memory behind a raw pointer is not owned by the stack frame, so the
    /// compiler does not track it: a load through it is a bitwise copy, and a
    /// store through it is a plain write which drops nothing.
    RawDeref,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Address {
    root: AddressRoot,
    projections: Interned<[Projection]>,
}

impl Address {
    fn new_root(root: AddressRoot, engine: &TrackedEngine) -> Self {
        Self { root, projections: engine.intern_unsized([]) }
    }

    #[must_use]
    pub fn new_error(engine: &TrackedEngine) -> Self { Self::new_root(AddressRoot::Error, engine) }

    #[must_use]
    pub fn new_variable(var_id: IRVariableID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Variable(var_id), engine)
    }

    #[must_use]
    pub fn new_parameter(parameter_id: ParameterID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Parameter(parameter_id), engine)
    }

    #[must_use]
    pub fn new_lambda_parameter(parameter_id: LambdaParameterID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::LambdaParameter(parameter_id), engine)
    }

    #[must_use]
    pub fn new_operation_handler_parameter(
        parameter_id: OperationHandlerParameterID,
        engine: &TrackedEngine,
    ) -> Self {
        Self::new_root(AddressRoot::OperationHandlerParameter(parameter_id), engine)
    }

    #[must_use]
    pub fn new_capture(capture_id: CaptureID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Capture(capture_id), engine)
    }

    /// Extends this address by one projection.
    pub fn add_projection(&mut self, projection: Projection, engine: &TrackedEngine) {
        let mut new_projections = Vec::with_capacity(self.projections.len() + 1);
        new_projections.extend(self.projections.iter().copied());
        new_projections.push(projection);
        self.projections = engine.intern_unsized(new_projections);
    }

    /// Returns this address extended by one projection.
    #[must_use]
    pub fn projected(&self, projection: Projection, engine: &TrackedEngine) -> Self {
        let mut address = self.clone();
        address.add_projection(projection, engine);
        address
    }

    pub fn add_tuple_index(&mut self, index: usize, engine: &TrackedEngine) {
        self.add_projection(Projection::Tuple(index), engine);
    }

    pub fn add_field(&mut self, field_id: FieldID, engine: &TrackedEngine) {
        self.add_projection(Projection::Field(field_id), engine);
    }

    pub fn add_deref(&mut self, engine: &TrackedEngine) {
        self.add_projection(Projection::Deref, engine);
    }

    pub fn add_raw_deref(&mut self, engine: &TrackedEngine) {
        self.add_projection(Projection::RawDeref, engine);
    }

    #[must_use]
    pub const fn root(&self) -> AddressRoot { self.root }

    /// Returns the local this address starts from, or `None` for an error
    /// address.
    ///
    /// The address may continue past the local through a dereference, so the
    /// selected memory is not necessarily part of the local.
    #[must_use]
    pub const fn local(&self) -> Option<Local> {
        match self.root {
            AddressRoot::Variable(variable) => Some(Local::Variable(variable)),
            AddressRoot::Parameter(parameter) => Some(Local::Parameter(parameter)),
            AddressRoot::LambdaParameter(parameter) => Some(Local::LambdaParameter(parameter)),
            AddressRoot::OperationHandlerParameter(parameter) => {
                Some(Local::OperationHandlerParameter(parameter))
            }
            AddressRoot::Capture(capture) => Some(Local::Capture(capture)),
            AddressRoot::Error => None,
        }
    }

    /// Returns the local this address selects a place in, or `None` when the
    /// address is an error address or goes through a dereference.
    ///
    /// Unlike [`Self::local`], the selected place is always part of the
    /// local's own storage, never memory reached through a pointer held in it.
    #[must_use]
    pub fn direct_local(&self) -> Option<Local> {
        if self.is_behind_deref() {
            return None;
        }

        self.local()
    }

    #[must_use]
    pub fn projections(&self) -> &[Projection] { &self.projections }

    /// Returns whether the place `other` selects lies within the place this
    /// address selects: both start from the same root, and the projections of
    /// this address are a prefix of those of `other`.
    ///
    /// Every address contains itself. `x` contains `x.0` and `*x`, but `x.0`
    /// contains neither `x` nor `x.1`.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        self.root == other.root && other.projections.starts_with(&self.projections)
    }

    /// Returns whether this address reaches memory through a pointer, that is,
    /// whether any of its projections is a dereference.
    #[must_use]
    pub fn is_behind_deref(&self) -> bool {
        self.projections.iter().any(|projection| projection.is_deref())
    }

    /// Returns the address of the pointer read by the first dereference in
    /// this address, or `None` when the address has no dereference.
    ///
    /// Everything before that dereference is storage of the current frame, so
    /// this is the longest prefix of the address which the frame owns.
    #[must_use]
    pub fn deref_base(&self, engine: &TrackedEngine) -> Option<Self> {
        let deref_index = self.projections.iter().position(|projection| projection.is_deref())?;
        Some(Self {
            root: self.root,
            projections: engine.intern_unsized(self.projections[..deref_index].to_vec()),
        })
    }

    /// Returns whether the last dereference in this address is of a
    /// reference, which makes the place borrowed memory. A place reached last
    /// through a raw pointer is untracked memory instead.
    #[must_use]
    pub fn is_behind_reference(&self) -> bool {
        matches!(
            self.projections.iter().rev().find(|projection| projection.is_deref()),
            Some(Projection::Deref)
        )
    }
}

impl Projection {
    /// Returns whether this projection reads a pointer and continues at the
    /// memory it points to.
    #[must_use]
    pub const fn is_deref(self) -> bool {
        match self {
            Self::Deref | Self::RawDeref => true,
            Self::Tuple(_) | Self::Field(_) => false,
        }
    }
}
