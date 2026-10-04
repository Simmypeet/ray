use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::{
    subst::Subst,
    ty::{Ty, effect_row::EffectLabel},
};

use crate::{
    ir_expr::IRExprID,
    visit::{
        TypeSite, TypeVisitor, TypeVisitorMut, TypeVisitorMutAsync, VisitType, VisitTypeMut,
        VisitTypeMutAsync,
    },
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

    /// Returns the effect row the `perform` introduces: the label of its
    /// effect alone, instantiated with the substitution of the `perform`.
    pub async fn effect_row(&self, engine: &TrackedEngine) -> Interned<Ty> {
        let label = EffectLabel::instantiated(self.effect_id, &self.substitution, engine).await;

        Ty::new_effect_row([label], None, engine)
    }
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

impl VisitTypeMutAsync for Perform {
    async fn visit_types_mut_async<V: TypeVisitorMutAsync>(
        &mut self,
        site: TypeSite,
        visitor: &mut V,
    ) {
        for ty in self.substitution.codomain_mut() {
            visitor.visit_type_mut_async(ty, site).await;
        }
    }
}
