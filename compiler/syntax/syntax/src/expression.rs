use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Numeric, Punctuation, irrefutable_pattern::IrrefutablePattern,
    r#type::Arrow,
};

abstract_tree::abstract_tree! {
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode
    )]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct LambdaParameterList {
        pub parameters: #[multi] IrrefutablePattern = ast::<IrrefutablePattern>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode
    )]
    pub struct Lambda {
        pub parameters: LambdaParameterList = ast::<LambdaParameterList>(),
        pub arrow: Arrow = ast::<Arrow>(),
        pub body: Expression = ast::<Expression>(),
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
    pub struct Binary {
        pub postfix: Postfix = ast::<Postfix>(),
        pub subsequent: #[multi] BinarySubsequent = ast::<BinarySubsequent>()
            .repeat()
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
    pub struct BinarySubsequent {
        pub operator: BinaryOperator = ast::<BinaryOperator>(),
        pub postfix: Postfix = ast::<Postfix>(),
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
    pub enum BinaryOperator {
        Assign(Punctuation = '='),
        Plus(Punctuation = '+'),
        Minus(Punctuation = '-'),
        Multiply(Punctuation = '*'),
        Divide(Punctuation = '/'),
        And(Keyword = expect::Keyword::And),
        Or(Keyword = expect::Keyword::Or),
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
    pub enum Literal {
        Boolean(Boolean = ast::<Boolean>()),
        Numeric(Numeric = expect::Numeric)
    }
}

abstract_tree::abstract_tree! {
    pub enum Leaf {
        Identifier(Identifier = expect::Identifier),
        Literal(Literal = ast::<Literal>()),
        Parenthesized(Parenthesized = ast::<Parenthesized>()),
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
    pub struct RefOf {
        pub dot: Punctuation = '.',
        pub asterisk: Punctuation = '&',
        pub mut_keyword: Keyword = expect::Keyword::Mut.optional(),
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
    pub struct Deref {
        pub dot: Punctuation = '.',
        pub asterisk: Punctuation = '*',
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
    pub enum PostfixOperator {
        Call(Call = ast::<Call>()),
        RefOf(RefOf = ast::<RefOf>()),
        Deref(Deref = ast::<Deref>()),
        TupleIndex(TupleIndex = ast::<TupleIndex>())
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
    pub struct TupleIndex {
        pub dot: Punctuation = '.',
        pub numeric: Numeric = expect::Numeric,
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
    pub struct Postfix {
        pub leaf: Leaf = ast::<Leaf>(),
        pub postfixes: #[multi] PostfixOperator = ast::<PostfixOperator>()
            .repeat()
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
    pub enum Boolean {
        True(Keyword = expect::Keyword::True),
        False(Keyword = expect::Keyword::False),
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
    pub struct IfElseCondition {
        pub expression: Expression = ast::<Expression>(),
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
    pub struct IfElseThenArm {
        pub then_colon: Punctuation = ':',
        pub expression: Expression = ast::<Expression>(),
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
    pub struct IfElseElseArm {
        pub else_keyword: Keyword = expect::Keyword::Else,
        pub else_colon: Punctuation = ':',
        pub expression: Expression = ast::<Expression>(),
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
    pub struct IfElse {
        pub if_keyword: Keyword = expect::Keyword::If,
        pub condition: IfElseCondition = ast::<IfElseCondition>(),
        pub then_arm: IfElseThenArm = ast::<IfElseThenArm>(),
        pub else_arm: IfElseElseArm = ast::<IfElseElseArm>(),
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
    pub enum Expression {
        Lambda(Lambda = ast::<Lambda>()),
        IfElse(IfElse = ast::<IfElse>()),
        Binary(Binary = ast::<Binary>()),
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
    pub struct Parenthesized {
        pub expressions: #[multi] Expression = ast::<Expression>()
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
    pub struct Call {
        pub arguments: Parenthesized = ast::<Parenthesized>()
    }
}
