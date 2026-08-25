//! Contains the queries for retrieving syntax items defined to a particular
//! symbol.

use qbice::{Decode, Encode, Query, StableHash};
use rayc_syntax::{
    def::{ParameterList, ReturnType},
    statement::Block,
};

use crate::GlobalSymbolID;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value((Option<ParameterList>, Option<ReturnType>))]
#[extend(by_val, name = get_def_signature_syntax)]
pub struct DefSignatureSyntaxKey {
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
