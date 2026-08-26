use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, r#type::Type};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Bracket)}
    pub struct TypeArgumentList {
        pub arguments: #[multi] Type = ast::<Type>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct PathSegment {
        pub identifier: Identifier = expect::Identifier,
        pub type_arguments: TypeArgumentList = ast::<TypeArgumentList>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Path {
        pub segments: #[multi] PathSegment = ast::<PathSegment>()
            .repeat_with_separator_at_least_once('.')
    }
}

#[cfg(test)]
mod test;
