use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Punctuation, expression::Expression,
    irrefutable_pattern::IrrefutablePattern, path::Path, r#type::Type,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct HandlerParameterList {
        pub parameters: #[multi] IrrefutablePattern = ast::<IrrefutablePattern>()
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
    pub struct HandlerOperation {
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub name: Identifier = expect::Identifier,
        pub parameter_list: HandlerParameterList = ast::<HandlerParameterList>(),
        pub block: Block = ast::<Block>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct HandlerBody {
        pub operations: #[multi] HandlerOperation = ast::<HandlerOperation>().line().repeat_all()
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
    pub struct Run {
        pub run_keyword: Keyword = expect::Keyword::Run,
        pub block: Block = ast::<Block>(),
        pub with_keyword: Keyword = expect::Keyword::With.new_line_significant(false),
        pub effect: Path = ast::<Path>(),
        pub handler_body: HandlerBody = ast::<HandlerBody>()
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
    pub struct Let {
        pub let_keyword: Keyword = expect::Keyword::Let,
        pub pattern: IrrefutablePattern = ast::<IrrefutablePattern>(),
        pub type_annotation: TypeAnnotation = ast::<TypeAnnotation>().optional(),
        pub assignment: VariableInitialization = ast::<VariableInitialization>().optional(),
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
    pub struct VariableInitialization {
        pub equals: Punctuation = '=',
        pub expression: Expression = ast::<Expression>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TypeAnnotation {
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
    pub struct Return {
        pub return_keyword: Keyword = expect::Keyword::Return,
        pub expression: Expression = ast::<Expression>().optional(),
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
    pub enum Statement {
        Run(Run = ast::<Run>()),
        Let(Let = ast::<Let>()),
        Expression(Expression = ast::<Expression>()),
        Return(Return = ast::<Return>())
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
    #{fragment = Fragment::Indentation}
    pub struct Block {
        pub statements: #[multi] Statement = ast::<Statement>()
            .line().repeat_all()
    }
}
