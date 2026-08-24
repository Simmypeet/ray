use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum CallTarget {
    Direct { function_id: GlobalSymbolID, subst: Subst },
    Lambda { callee: TypedExprID },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Call {
    target: CallTarget,
    arguments: Vec<TypedExprID>,
}

impl Call {
    #[must_use]
    pub const fn new_direct(
        function_id: GlobalSymbolID,
        arguments: Vec<TypedExprID>,
        subst: Subst,
    ) -> Self {
        Self { target: CallTarget::Direct { function_id, subst }, arguments }
    }

    #[must_use]
    pub const fn new_lambda(callee: TypedExprID, arguments: Vec<TypedExprID>) -> Self {
        Self { target: CallTarget::Lambda { callee }, arguments }
    }

    #[must_use]
    pub const fn target(&self) -> &CallTarget { &self.target }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExprID] { &self.arguments }
}

impl MutSubstitutable for Call {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        match &mut self.target {
            CallTarget::Direct { subst: call_subst, .. } => {
                call_subst.apply_mut_subst(subst, engine);
            }
            CallTarget::Lambda { .. } => {}
        }
    }
}
