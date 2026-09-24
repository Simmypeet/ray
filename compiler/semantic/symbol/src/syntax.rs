//! Contains the queries for retrieving syntax items defined to a particular
//! symbol.

use qbice::{Decode, Encode, Query, StableHash};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    given::GivenParameterList,
    kind::KindAscription,
    path::Path,
    statement::Block,
    r#struct::StructBody,
    r#type::Type,
    where_clause::WhereClause,
};

use crate::GlobalSymbolID;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<ParameterList>)]
#[extend(by_val, name = get_parameter_list_syntax)]
pub struct ParameterListSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<ReturnType>)]
#[extend(by_val, name = get_return_type_syntax)]
pub struct ReturnTypeSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<EffectRowAnnotation>)]
#[extend(by_val, name = get_effect_row_syntax)]
pub struct EffectRowSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Block>)]
#[extend(by_val, name = get_def_body_syntax)]
pub struct DefBodySyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the body declared by a struct symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<StructBody>)]
#[extend(by_val, name = get_struct_body_syntax)]
pub struct StructBodySyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves whether a struct symbol is declared with the `@linear`
/// attribute. A linear struct never has a `Drop` instance, so its values must
/// be consumed explicitly.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(bool)]
#[extend(by_val, name = is_linear_struct)]
pub struct LinearStructKey {
    pub symbol_id: GlobalSymbolID,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(bool)]
#[extend(by_val, name = is_variadic_def)]
pub struct VariadicDefKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the explicitly declared type parameters of a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<TypeParameterList>)]
#[extend(by_val, name = get_type_parameter_list_syntax)]
pub struct TypeParameterListSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the explicitly declared given parameters of a symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<GivenParameterList>)]
#[extend(by_val, name = get_given_parameter_list_syntax)]
pub struct GivenParameterListSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the trait reference declared by an instance symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Path>)]
#[extend(by_val, name = get_instance_trait_syntax)]
pub struct InstanceTraitSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the marker path declared by a marker-implementation symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Path>)]
#[extend(by_val, name = get_marker_implementation_marker_syntax)]
pub struct MarkerImplementationMarkerSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves whether a marker implementation has a leading `!`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(bool)]
#[extend(by_val, name = is_negative_marker_implementation)]
pub struct NegativeMarkerImplementationSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the implementor type declared by a marker-implementation symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Type>)]
#[extend(by_val, name = get_marker_implementation_type_syntax)]
pub struct MarkerImplementationTypeSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the type definition declared by an instance associated type.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<Type>)]
#[extend(by_val, name = get_type_definition_syntax)]
pub struct TypeDefinitionSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the explicitly declared where clause of a symbol.
///
/// Only valid for symbol kinds supporting
/// [`has_where_clause`](crate::symbol_kind::SymbolKind::has_where_clause).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<WhereClause>)]
#[extend(by_val, name = get_where_clause_syntax)]
pub struct WhereClauseSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves the explicitly declared result-kind ascription of an
/// associated type.
///
/// Only valid for trait and instance associated-type symbols. An omitted
/// ascription returns `None`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<KindAscription>)]
#[extend(by_val, name = get_kind_ascription_syntax)]
pub struct KindAscriptionSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}
