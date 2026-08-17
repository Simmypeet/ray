use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::Ty,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Variable {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

pub type VariableID = ID<Variable>;

impl Variable {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

impl MutSubstitutable for Variable {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.ty.apply_in_place(subst, engine);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct VariableMap {
    variables: Arena<Variable>,
}

impl VariableMap {
    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> ID<Variable> {
        self.variables.insert(variable)
    }

    #[must_use]
    pub fn get_variable(&self, id: VariableID) -> &Variable {
        self.variables.get(id).expect("VariableID should be valid")
    }
}

impl MutSubstitutable for VariableMap {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        for variable in self.variables.items_mut() {
            variable.apply_mut_subst(subst, engine);
        }
    }
}
