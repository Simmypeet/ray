use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, Keyword, Punctuation, path::Path};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct GivenParameter {
        pub name: Identifier = expect::Identifier,
        pub colon: Punctuation = ':',
        pub trait_reference: Path = ast::<Path>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct GivenParameters {
        pub parameters: #[multi] GivenParameter = ast::<GivenParameter>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct GivenParameterList {
        pub given_keyword: Keyword = expect::Keyword::Given,
        pub parameters: GivenParameters = ast::<GivenParameters>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct GivenArgumentName {
        pub name: Identifier = expect::Identifier,
        pub equals: Punctuation = '='
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct GivenArgument {
        pub name: GivenArgumentName = ast::<GivenArgumentName>().optional(),
        pub dictionary: Path = ast::<Path>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct GivenArguments {
        pub arguments: #[multi] GivenArgument = ast::<GivenArgument>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct GivenArgumentList {
        pub given_keyword: Keyword = expect::Keyword::Given,
        pub arguments: GivenArguments = ast::<GivenArguments>()
    }
}
