use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

/// The verified correspondence between one trait definition and its instance
/// implementation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct InstanceDef {
    trait_def_id: GlobalSymbolID,
    implementation_id: GlobalSymbolID,
    /// Maps polymorphic variables owned by the enclosing
    /// [`SymbolKind::Trait`](rayc_symbol::symbol_kind::SymbolKind::Trait) to
    /// the types supplied by the instance's trait reference, and maps local
    /// polymorphic variables owned by the corresponding
    /// [`SymbolKind::TraitDef`](rayc_symbol::symbol_kind::SymbolKind::TraitDef)
    /// to the positionally corresponding polymorphic variables owned by this
    /// [`SymbolKind::InstanceDef`](rayc_symbol::symbol_kind::SymbolKind::InstanceDef).
    poly_var_substitution: Subst,
}

impl InstanceDef {
    #[must_use]
    pub const fn new(
        trait_def_id: GlobalSymbolID,
        instance_def_id: GlobalSymbolID,
        poly_var_substitution: Subst,
    ) -> Self {
        Self { trait_def_id, implementation_id: instance_def_id, poly_var_substitution }
    }

    #[must_use]
    pub const fn trait_def_id(&self) -> GlobalSymbolID { self.trait_def_id }

    #[must_use]
    pub const fn instance_def_id(&self) -> GlobalSymbolID { self.implementation_id }

    /// Returns the substitution from the trait definition's polymorphic
    /// variables to the instance definition's types.
    ///
    /// Variables owned by the enclosing
    /// [`SymbolKind::Trait`](rayc_symbol::symbol_kind::SymbolKind::Trait) map
    /// to the types supplied by the instance's trait reference. Local
    /// variables owned by the corresponding
    /// [`SymbolKind::TraitDef`](rayc_symbol::symbol_kind::SymbolKind::TraitDef)
    /// map to the positionally corresponding variables owned by this
    /// [`SymbolKind::InstanceDef`](rayc_symbol::symbol_kind::SymbolKind::InstanceDef).
    #[must_use]
    pub const fn poly_var_substitution(&self) -> &Subst { &self.poly_var_substitution }
}

/// Verifies and retrieves an instance-definition symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<InstanceDef>)]
#[extend(by_val, name = get_instance_def)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
