use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Keyword, Punctuation, expression::Expression, irrefutable_pattern::IrrefutablePattern,
    r#type::Type,
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
    pub struct Let {
        pub let_keyword: Keyword = expect::Keyword::Let,
        pub pattern: IrrefutablePattern = ast::<IrrefutablePattern>(),
        pub type_annotation: TypeAnnotation = ast::<TypeAnnotation>().commit_if(':'.lookahead()),
        pub assignment: VariableInitialization =
            ast::<VariableInitialization>().commit_if('='.lookahead()),
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
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Break {
        pub break_keyword: Keyword = expect::Keyword::Break,
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Continue {
        pub continue_keyword: Keyword = expect::Keyword::Continue,
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
    #{label = rayc_parser::expect::Label::Statement}
    pub enum Statement {
        Let(Let = ast::<Let>()),
        Break(Break = ast::<Break>()),
        Continue(Continue = ast::<Continue>()),
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
