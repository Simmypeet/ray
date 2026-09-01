use qbice::{Decode, Encode, Identifiable, StableHash};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::function::MonoFunctionID;

/// Identifies one concrete instantiation of a source definition.
///
/// This is intentionally shaped like the key of the future incremental `MonoIR`
/// query. Global references stored in one fragment therefore also describe the
/// other fragments required by the eventual program orchestrator.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoDefInstance {
    def_id: GlobalSymbolID,
    substitution: Subst,
}

impl MonoDefInstance {
    #[must_use]
    pub const fn new(def_id: GlobalSymbolID, substitution: Subst) -> Self {
        Self { def_id, substitution }
    }

    #[must_use]
    pub const fn def_id(&self) -> GlobalSymbolID { self.def_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}

/// Identifies one concrete instantiation of an effect declaration.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MonoEffectInstance {
    effect_id: GlobalSymbolID,
    substitution: Subst,
}

impl MonoEffectInstance {
    #[must_use]
    pub const fn new(effect_id: GlobalSymbolID, substitution: Subst) -> Self {
        Self { effect_id, substitution }
    }

    #[must_use]
    pub const fn effect_id(&self) -> GlobalSymbolID { self.effect_id }

    #[must_use]
    pub const fn substitution(&self) -> &Subst { &self.substitution }
}

/// References either a nested function in the current fragment or the root
/// function of another concrete definition fragment.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum FunctionReference {
    Local(MonoFunctionID),
    Global(MonoDefInstance),
}
