use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_type::subst::{MutSubstitutable, Subst};

use crate::{
    typed_expr::{SubExprs, TypedExprID},
    typed_function::TypedFunctionID,
};

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct RunWith {
    effect: GlobalSymbolID,
    effect_substitution: Subst,
    body: TypedFunctionID,

    /// Each of the symbol IDs in this map is the operation handler under the
    /// `effect`
    operation_handlers: FxHashMap<SymbolID, TypedFunctionID>,
}

impl RunWith {
    #[must_use]
    pub const fn new(
        effect: GlobalSymbolID,
        effect_substitution: Subst,
        body: TypedFunctionID,
        operation_handlers: FxHashMap<SymbolID, TypedFunctionID>,
    ) -> Self {
        Self { effect, effect_substitution, body, operation_handlers }
    }

    #[must_use]
    pub const fn effect(&self) -> GlobalSymbolID { self.effect }

    #[must_use]
    pub const fn effect_substitution(&self) -> &Subst { &self.effect_substitution }

    #[must_use]
    pub const fn body(&self) -> TypedFunctionID { self.body }

    #[must_use]
    pub fn operation_handlers(&self) -> impl ExactSizeIterator<Item = TypedFunctionID> + '_ {
        self.operation_handlers.values().copied()
    }
}

impl SubExprs for RunWith {
    fn sub_exprs(&self) -> impl Iterator<Item = TypedExprID> { std::iter::empty() }
}

impl MutSubstitutable for RunWith {
    fn apply_mut_subst(&mut self, subst: &Subst, engine: &TrackedEngine) {
        self.effect_substitution.apply_mut_subst(subst, engine);
    }
}
