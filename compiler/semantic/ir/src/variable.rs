use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Variable {
    ty: Interned<Ty>,
    span: Option<RelativeSpan>,
}

impl Variable {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span: Some(span) } }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct VariableMap {
    variables: Arena<Variable>,
}

impl VariableMap {
    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> ID<Variable> {
        self.variables.insert(variable)
    }
}
