use qbice::{Decode, Encode, Identifiable, StableHash, storage::intern::Interned};
use rayc_arena::ID;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::Parameter;

use crate::{expression::Expression, variable::Variable};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum AddressRoot {
    Variable(ID<Variable>),
    Parameter(ID<Parameter>),
    Deref(ID<Expression>),
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode, Identifiable,
)]
pub enum Projection {
    Deref,
    Tuple(usize),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Address {
    root: AddressRoot,
    projections: Interned<[Projection]>,
}

impl Address {
    pub fn new_root(root: AddressRoot, engine: &TrackedEngine) -> Self {
        Self { root, projections: engine.intern_unsized([]) }
    }

    pub fn new_variable(var_id: ID<Variable>, engine: &TrackedEngine) -> Self {
        Self::new_root(AddressRoot::Variable(var_id), engine)
    }

    pub fn add_projection(&mut self, projection: Projection, engine: &TrackedEngine) {
        let mut new_projections = Vec::with_capacity(self.projections.len() + 1);
        new_projections.extend(self.projections.iter().copied());
        new_projections.push(projection);
        self.projections = engine.intern_unsized(new_projections);
    }

    pub fn add_deref(&mut self, engine: &TrackedEngine) {
        self.add_projection(Projection::Deref, engine);
    }

    pub fn add_tuple_index(&mut self, index: usize, engine: &TrackedEngine) {
        self.add_projection(Projection::Tuple(index), engine);
    }
}
