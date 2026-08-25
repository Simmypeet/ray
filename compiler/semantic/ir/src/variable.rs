use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::visit::{TypeVisitor, VisitType};

/// Identifies a local variable stored in a function's variable arena.
pub type VariableID = ID<Variable>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Variable {
    ty: Interned<Ty>,
    span: RelativeSpan,
}

impl Variable {
    #[must_use]
    pub const fn new(ty: Interned<Ty>, span: RelativeSpan) -> Self { Self { ty, span } }

    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct VariableMap {
    variables: Arena<Variable>,
}

impl VariableMap {
    #[must_use]
    pub fn insert_variable(&mut self, variable: Variable) -> VariableID {
        self.variables.insert(variable)
    }

    #[must_use]
    pub fn get_variable(&self, id: VariableID) -> &Variable {
        self.variables.get(id).expect("Variable should exist")
    }

    #[must_use]
    pub fn variables(&self) -> impl ExactSizeIterator<Item = (VariableID, &Variable)> {
        self.variables.iter()
    }
}

impl VisitType for Variable {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) { visitor.visit_type(&self.ty); }
}

impl VisitType for VariableMap {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        for (_, variable) in self.variables() {
            variable.visit_types(visitor);
        }
    }
}
