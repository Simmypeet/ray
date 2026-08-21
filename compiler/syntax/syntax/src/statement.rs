use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Keyword, Punctuation, expression::Expression, irrefutable_pattern::IrrefutablePattern,
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
        pub equals: Punctuation = '=',
        pub expression: Expression = ast::<Expression>()
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
