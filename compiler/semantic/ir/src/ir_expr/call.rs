use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::{
    ir_expr::ExpressionID,
    visit::{TypeVisitor, VisitType},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub enum CallTarget {
    Direct { function_id: GlobalSymbolID, subst: Subst },
    Lambda { callee: ExpressionID },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Call {
    target: CallTarget,
    arguments: Vec<ExpressionID>,
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
        arguments: Vec<ExpressionID>,
        subst: Subst,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments }
    }

    #[must_use]
    pub const fn new_lambda(callee: ExpressionID, arguments: Vec<ExpressionID>) -> Self {
        Self { target: CallTarget::Lambda { callee }, arguments }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[ExpressionID] { &self.arguments }
}
