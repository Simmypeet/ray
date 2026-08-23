use qbice::{Decode, Encode, StableHash};
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::typed_expr::TypedExprID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct Call {
    function_id: GlobalSymbolID,
    arguments: Vec<TypedExprID>,
    subst: Subst,
}

impl Call {
    #[must_use]
    pub const fn new(
        function_id: GlobalSymbolID,
        arguments: Vec<TypedExprID>,
        subst: Subst,
    ) -> Self {
        Self { function_id, arguments, subst }
    }

    #[must_use]
    pub const fn function_id(&self) -> GlobalSymbolID { self.function_id }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExprID] { &self.arguments }

    #[must_use]
    pub const fn subst(&self) -> &Subst { &self.subst }
}

impl MutSubstitutable for Call {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.subst.apply_mut_subst(subst, engine);
    }
}
