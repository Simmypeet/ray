use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Punctuation,
    irrefutable_pattern::IrrefutablePattern,
    statement::Block,
    r#type::{Arrow, Type},
};

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    pub struct DefSignature {
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub name: Identifier = expect::Identifier,
        pub parameter_list: ParameterList = ast::<ParameterList>(),
        pub return_type: ReturnType = ast::<ReturnType>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct ParameterList {
        pub parameters: #[multi] Parameter = ast::<Parameter>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    pub struct Parameter {
        pub irrefutable_pattern: IrrefutablePattern
            = ast::<IrrefutablePattern>(),
        pub colon: Punctuation = ':',
        pub r#type: Type = ast::<Type>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    pub struct ReturnType {
        pub arrow: Arrow = ast::<Arrow>(),
        pub r#type: Type = ast::<Type>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    pub struct Def {
        pub signature: DefSignature = ast::<DefSignature>(),
        pub block: Block = ast::<Block>()
    }
}
