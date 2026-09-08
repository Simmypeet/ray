use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

/// The correspondence and polymorphic-variable mapping between a trait
/// associated method or type and its instance implementation.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct InstanceMember {
    trait_member_id: GlobalSymbolID,
    implementation_id: GlobalSymbolID,
    /// Maps enclosing trait variables to the instance's trait arguments and
    /// local trait-member variables to corresponding instance-member variables.
    poly_var_substitution: Subst,
}

impl InstanceMember {
    #[must_use]
    pub const fn new(
        trait_member_id: GlobalSymbolID,
        instance_member_id: GlobalSymbolID,
        poly_var_substitution: Subst,
    ) -> Self {
        Self { trait_member_id, implementation_id: instance_member_id, poly_var_substitution }
    }

    #[must_use]
    pub const fn trait_member_id(&self) -> GlobalSymbolID { self.trait_member_id }

    #[must_use]
    pub const fn instance_member_id(&self) -> GlobalSymbolID { self.implementation_id }

    /// Returns the substitution from trait-member polymorphic variables to
    /// instance-member types. Supports both associated methods and types;
    /// local variables correspond by position, independently of their names.
    #[must_use]
    pub const fn poly_var_substitution(&self) -> &Subst { &self.poly_var_substitution }
}

/// Checks compatibility and retrieves an instance associated method or type.
///
/// Accepts `SymbolKind::InstanceDef` and `SymbolKind::InstanceType`.
/// Diagnostics report incompatibility; an incompatible polymorphic signature
/// yields an empty substitution.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<InstanceMember>)]
#[extend(by_val, name = get_instance_member)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
