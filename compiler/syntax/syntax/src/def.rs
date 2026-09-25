use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Ext, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Punctuation,
    effect::TypeParameterList,
    effect_row::EffectRowAnnotation,
    given::GivenParameterList,
    irrefutable_pattern::IrrefutablePattern,
    statement::Block,
    r#type::{Arrow, Type},
    where_clause::WhereClause,
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
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub parameter_list: ParameterList = ast::<ParameterList>(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub return_type: ReturnType = ast::<ReturnType>().optional(),
        pub effect_row: EffectRowAnnotation = ast::<EffectRowAnnotation>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional()
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
        pub entries: #[multi] ParameterEntry = ast::<ParameterEntry>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum ParameterEntry {
        Parameter(Parameter = ast::<Parameter>()),
        Ellipsis(Ellipsis = ast::<Ellipsis>())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Ellipsis {
        pub first: Punctuation = '.',
        pub second: Punctuation = '.'.no_prior_insignificant(),
        pub third: Punctuation = '.'.no_prior_insignificant()
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
        pub r#type: ParameterType = ast::<ParameterType>()
    }
}

// Callable sugar is available only as a complete definition parameter
// annotation.
abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum ParameterType {
        CallableSugar(Callable = ast::<Callable>()),
        Type(Type = ast::<Type>())
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
    pub struct CallableReturnType {
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
    pub struct Callable {
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub parameters: CallableParameterList = ast::<CallableParameterList>(),
        pub return_type: CallableReturnType = ast::<CallableReturnType>().optional(),
        pub effect_row: EffectRowAnnotation = ast::<EffectRowAnnotation>().optional()
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
    pub struct CallableParameterList {
        pub parameters: #[multi] Type = ast::<Type>()
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
