//!  Contains the definition of the [`Kind`] enum.

use std::{fmt::Debug, hash::Hash, sync::Arc};

use qbice::{Decode, Encode, Identifiable, Query, StableHash};
use rayc_target::{Global, TargetID};

use crate::SymbolID;

/// An enumeration used to identify the kind of a symbol in the Ray. This
/// value should be set to every symbol that is defined in the compilation
/// target.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Encode,
    Decode,
    StableHash,
    Identifiable,
)]
#[allow(missing_docs)]
pub enum SymbolKind {
    Def,
    Effect,
    EffectOperation,
    ExternDef,
    Instance,
    InstanceDef,
    InstanceType,
    Marker,
    MarkerImplementation,
    Module,
    Strut,
    Trait,
    TraitDef,
    TraitType,
}

/// The key type used with [`TrackedEngine`] to access the kind of a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Encode, Decode, Query, StableHash,
)]
#[value(SymbolKind)]
#[extend(name = get_symbol_kind, by_val)]
pub struct Key {
    /// The global ID of the symbol to get the kind for.
    pub symbol_id: Global<SymbolID>,
}

impl SymbolKind {
    /// Checks if this kind of symbol has a [`Member`] component.
    #[must_use]
    pub const fn has_member(&self) -> bool {
        matches!(self, Self::Effect | Self::Instance | Self::Module | Self::Trait)
    }

    /// Checks if this kind of symbol has a definition body.
    #[must_use]
    pub const fn has_def_body(&self) -> bool { matches!(self, Self::Def | Self::InstanceDef) }

    /// Checks if this kind of symbol owns a polymorphic-variable map.
    #[must_use]
    pub const fn has_poly_var_map(&self) -> bool {
        matches!(
            self,
            Self::Def
                | Self::Effect
                | Self::Instance
                | Self::InstanceDef
                | Self::InstanceType
                | Self::MarkerImplementation
                | Self::Strut
                | Self::Trait
                | Self::TraitDef
                | Self::TraitType
        )
    }

    /// Checks if this kind of symbol has a parameter list
    #[must_use]
    pub const fn has_parameter_list(&self) -> bool {
        matches!(
            self,
            Self::Def
                | Self::EffectOperation
                | Self::ExternDef
                | Self::InstanceDef
                | Self::TraitDef
        )
    }

    /// Checks if this kind of symbol has a parameter list
    #[must_use]
    pub const fn has_return_type(&self) -> bool {
        matches!(
            self,
            Self::Def
                | Self::EffectOperation
                | Self::ExternDef
                | Self::InstanceDef
                | Self::TraitDef
        )
    }

    /// Checks if this kind of symbol has a parameter list
    #[must_use]
    pub const fn has_effect_row_annotation(&self) -> bool {
        matches!(self, Self::Def | Self::InstanceDef | Self::TraitDef)
    }

    /// Checks if this kind of symbol supports a where clause.
    #[must_use]
    pub const fn has_where_clause(&self) -> bool {
        match self {
            Self::Def
            | Self::Effect
            | Self::Instance
            | Self::InstanceDef
            | Self::InstanceType
            | Self::MarkerImplementation
            | Self::Strut
            | Self::Trait
            | Self::TraitDef
            | Self::TraitType => true,
            Self::EffectOperation | Self::ExternDef | Self::Marker | Self::Module => false,
        }
    }

    /// Returns the human-readable string representation of this symbol kind.
    #[must_use]
    pub const fn str(&self) -> &'static str {
        match self {
            Self::Def => "def",
            Self::Effect => "effect",
            Self::EffectOperation => "effect operation",
            Self::ExternDef => "extern def",
            Self::Instance => "instance",
            Self::InstanceDef => "instance def",
            Self::InstanceType => "instance type",
            Self::Marker => "marker",
            Self::MarkerImplementation => "marker implementation",
            Self::Module => "module",
            Self::Strut => "struct",
            Self::Trait => "trait",
            Self::TraitDef => "trait def",
            Self::TraitType => "trait type",
        }
    }
}

/// A query that returns all the symbol IDs in a given target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_symbol_ids, by_val)]
pub struct AllSymbolIDs {
    pub target: TargetID,
}

/// Retrieves all the def symbol IDs in a given target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_def_with_body_ids, by_val)]
pub struct AllDefWithBodyIDs {
    pub target: TargetID,
}

/// Retrieves all instance symbol IDs in a given target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_instance_ids, by_val)]
pub struct AllInstanceIDs {
    pub target: TargetID,
}

/// Retrieves all callable definition and effect-operation IDs in a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Arc<[SymbolID]>)]
#[extend(name = get_all_callable_def_ids, by_val)]
pub struct AllCallableDefIDs {
    pub target: TargetID,
}
