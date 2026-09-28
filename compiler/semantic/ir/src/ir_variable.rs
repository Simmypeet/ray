use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_arena::{Arena, ID};
use rayc_lexical::tree::RelativeSpan;
use rayc_type::ty::Ty;

use crate::{
    scope::ScopeID,
    visit::{TypeSite, TypeVisitor, TypeVisitorMut, VisitType, VisitTypeMut},
};

/// Identifies a local variable stored in a function's variable arena.
pub type IRVariableID = ID<IRVariable>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct IRVariable {
    ty: Interned<Ty>,
    span: RelativeSpan,
    scope_id: ScopeID,

    /// The position of this variable among its function's variables, in the
    /// order they were declared. Later declarations are dropped first.
    declaration_order: usize,
}

impl IRVariable {
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn scope_id(&self) -> ScopeID { self.scope_id }

    #[must_use]
    pub const fn declaration_order(&self) -> usize { self.declaration_order }
}

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default)]
pub struct IRVariableMap {
    variables: Arena<IRVariable>,
}

impl IRVariableMap {
    /// Declares a new variable after every variable declared so far.
    #[must_use]
    pub(crate) fn insert_variable(
        &mut self,
        ty: Interned<Ty>,
        span: RelativeSpan,
        scope_id: ScopeID,
    ) -> IRVariableID {
        let declaration_order = self.variables.len();
        self.variables.insert(IRVariable { ty, span, scope_id, declaration_order })
    }

    #[must_use]
    pub fn get_variable(&self, id: IRVariableID) -> &IRVariable {
        self.variables.get(id).expect("Variable should exist")
    }

    #[must_use]
    pub fn variables(&self) -> impl ExactSizeIterator<Item = (IRVariableID, &IRVariable)> {
        self.variables.iter()
    }
}

impl VisitType for IRVariable {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type(&self.ty, site);
    }
}

impl VisitType for IRVariableMap {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for (_, variable) in self.variables() {
            variable.visit_types(site, visitor);
        }
    }
}

impl VisitTypeMut for IRVariable {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        visitor.visit_type_mut(&mut self.ty, site);
    }
}

impl VisitTypeMut for IRVariableMap {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        for (_, variable) in self.variables.iter_mut() {
            variable.visit_types_mut(site, visitor);
        }
    }
}
