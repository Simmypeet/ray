use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::{
    ir_expr::IRExprID,
    visit::{TypeSite, TypeVisitor, TypeVisitorMut, VisitType, VisitTypeMut},
};

/// Invokes an operation of the dynamically nearest matching effect handler.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Perform {
    effect_id: GlobalSymbolID,
    operation_id: GlobalSymbolID,
    arguments: Vec<IRExprID>,
    substitution: Subst,
}

impl Perform {
    #[must_use]
    pub const fn new(
        effect_id: GlobalSymbolID,
        operation_id: GlobalSymbolID,
        arguments: Vec<IRExprID>,
        substitution: Subst,
    ) -> Self {
        Self { effect_id, operation_id, arguments, substitution }
    }

    #[must_use]
    pub const fn effect_id(&self) -> GlobalSymbolID { self.effect_id }

    #[must_use]
    pub const fn operation_id(&self) -> GlobalSymbolID { self.operation_id }

    #[must_use]
    pub fn arguments(&self) -> &[IRExprID] { &self.arguments }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}

impl VisitType for Perform {
    fn visit_types<V: TypeVisitor>(&self, site: TypeSite, visitor: &mut V) {
        for ty in self.substitution.codomain() {
            visitor.visit_type(ty, site);
        }
    }
}

impl VisitTypeMut for Perform {
    fn visit_types_mut<V: TypeVisitorMut>(&mut self, site: TypeSite, visitor: &mut V) {
        for ty in self.substitution.codomain_mut() {
            visitor.visit_type_mut(ty, site);
        }
    }
}
