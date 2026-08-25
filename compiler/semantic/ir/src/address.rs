use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;

use crate::{
    ir_expr::IRExprID,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_variable::IRVariableID,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum AddressRoot {
    Error,
    Variable(IRVariableID),
    Parameter(ParameterID),
    LambdaParameter(LambdaParameterID),
    Capture(CaptureID),
    Deref(IRExprID),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Projection {
    Tuple(usize),
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
    pub fn new_capture(capture_id: CaptureID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Capture(capture_id), engine)
    }

    #[must_use]
    pub fn new_deref(expression_id: IRExprID, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Deref(expression_id), engine)
    }

    fn add_projection(&mut self, projection: Projection, engine: &TrackedEngine) {
        let mut new_projections = Vec::with_capacity(self.projections.len() + 1);
        new_projections.extend(self.projections.iter().copied());
        new_projections.push(projection);
        self.projections = engine.intern_unsized(new_projections);
    }

    pub fn add_tuple_index(&mut self, index: usize, engine: &TrackedEngine) {
        self.add_projection(Projection::Tuple(index), engine);
    }

    #[must_use]
    pub const fn root(&self) -> AddressRoot { self.root }

    #[must_use]
    pub fn projections(&self) -> &[Projection] { &self.projections }
}
