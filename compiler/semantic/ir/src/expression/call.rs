use qbice::{Decode, Encode, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::expression::ExpressionID;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, StableHash, Encode, Decode)]
pub struct Call {
    function_id: GlobalSymbolID,
    arguments: Vec<ExpressionID>,
    subst: Subst,
}

impl Call {
    #[must_use]
    pub const fn new(
        function_id: GlobalSymbolID,
        arguments: Vec<ExpressionID>,
        subst: Subst,
    ) -> Self {
        Self { function_id, arguments, subst }
    }

    #[must_use]
    pub const fn function_id(&self) -> GlobalSymbolID { self.function_id }

    #[must_use]
    pub fn arguments(&self) -> &[ExpressionID] { &self.arguments }

    #[must_use]
    pub const fn subst(&self) -> &Subst { &self.subst }
}
