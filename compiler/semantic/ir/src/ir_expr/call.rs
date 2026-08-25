use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

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
    }
}

impl Call {
    #[must_use]
    pub const fn new_direct(
        function_id: GlobalSymbolID,
        arguments: Vec<IRExprID>,
        subst: Subst,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments }
    }

    #[must_use]
    pub const fn new_lambda(callee: IRExprID, arguments: Vec<IRExprID>) -> Self {
        Self { target: CallTarget::Lambda { callee }, arguments }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[IRExprID] { &self.arguments }
}
