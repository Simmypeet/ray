//! Contains the queries for retrieving syntax items defined to a particular
//! symbol.

use qbice::{Decode, Encode, Query, StableHash};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    effect::TypeParameterList,
    statement::Block,
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
#[value(Option<Block>)]
#[extend(by_val, name = get_def_body_syntax)]
pub struct DefBodySyntaxKey {
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

/// Retrieves the explicitly declared type parameters of an effect symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Option<TypeParameterList>)]
#[extend(by_val, name = get_effect_type_parameter_syntax)]
pub struct EffectTypeParameterSyntaxKey {
    pub symbol_id: GlobalSymbolID,
}
