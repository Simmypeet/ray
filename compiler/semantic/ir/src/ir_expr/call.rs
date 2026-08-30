use qbice::{Decode, Encode, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::{subst::Subst, ty::Ty};

use crate::{
    ir_expr::IRExprID,
    visit::{TypeVisitor, VisitType},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum CallTarget {
    Direct { function_id: GlobalSymbolID, subst: Subst },
    Lambda { callee: IRExprID },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Call {
    target: CallTarget,
    arguments: Vec<IRExprID>,
    effect: Interned<Ty>,
}

impl VisitType for Call {
    fn visit_types<V: TypeVisitor>(&self, visitor: &mut V) {
        match &self.target {
            CallTarget::Direct { subst, .. } => {
                for ty in subst.codomain() {
                    visitor.visit_type(ty);
                }
            }
            CallTarget::Lambda { .. } => {}
        }
        visitor.visit_type(&self.effect);
    }
}

impl Call {
    #[must_use]
    pub const fn new_direct(
        function_id: GlobalSymbolID,
        arguments: Vec<IRExprID>,
        subst: Subst,
        effect: Interned<Ty>,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments, effect }
    }

    #[must_use]
    pub const fn new_lambda(
        callee: IRExprID,
        arguments: Vec<IRExprID>,
        effect: Interned<Ty>,
    ) -> Self {
        Self { target: CallTarget::Lambda { callee }, arguments, effect }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[IRExprID] { &self.arguments }

    /// Returns the conservative ambient effect row at this invocation site.
    #[must_use]
    pub const fn effect(&self) -> &Interned<Ty> { &self.effect }
}
