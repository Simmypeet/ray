use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Ext, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Numeric, Punctuation, String as StringToken,
    irrefutable_pattern::IrrefutablePattern,
    path::Path,
    statement::Block,
    r#type::{Arrow, Type},
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
    pub struct RunWith {
        pub run_keyword: Keyword = expect::Keyword::Run,
        pub block: Block = ast::<Block>(),
        pub with_keyword: Keyword = expect::Keyword::With.new_line_significant(false),
        pub effect: Path = ast::<Path>(),
        pub handler_body: HandlerBody = ast::<HandlerBody>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode
    )]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct ClosureParameterList {
        pub parameters: #[multi] IrrefutablePattern = ast::<IrrefutablePattern>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(
        Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode
    )]
    pub struct Closure {
        pub parameter_list: ClosureParameterList = ast::<ClosureParameterList>(),
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
    pub struct Equal {
        pub first: Punctuation = '=',
        pub second: Punctuation = '='.no_prior_insignificant()
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
    pub struct NotEqual {
        pub exclamation: Punctuation = '!',
        pub equals: Punctuation = '='.no_prior_insignificant()
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
        pub cast: Cast = ast::<Cast>(),
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
        pub cast: Cast = ast::<Cast>(),
    }
}

// `-x as int64` casts `-x`, and `a + b as int64` casts only `b`: a cast binds
// tighter than every binary operator, but looser than a prefix operator.
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
    pub struct Cast {
        pub prefix: Prefix = ast::<Prefix>(),
        pub casts: #[multi] CastTarget = ast::<CastTarget>().repeat()
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
    pub struct CastTarget {
        pub as_keyword: Keyword = expect::Keyword::As,
        pub ty: Type = ast::<Type>(),
    }
}

// `-x.0` negates `x.0`, and `move x.0` moves `x.0`: a prefix operator binds
// looser than every postfix operator.
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
    pub struct Prefix {
        pub operators: #[multi] PrefixOperator = ast::<PrefixOperator>().repeat(),
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
    pub enum PrefixOperator {
        /// Negates a signed numeric operand.
        Negate(Punctuation = '-'),
        /// Moves out of the operand, even when its type is `Copy`.
        Move(Keyword = expect::Keyword::Move),
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
        Equal(Equal = ast::<Equal>()),
        NotEqual(NotEqual = ast::<NotEqual>()),
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
        Numeric(NumericLiteral = ast::<NumericLiteral>()),
        String(StringToken = expect::String)
    }
}

// The suffix must follow the digits directly: `23i8` is a suffixed literal,
// while `23 i8` is not. `isize` and `usize` are keywords, whereas the other
// suffixes are plain identifiers.
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
    pub enum NumericSuffix {
        I8(Identifier = expect::IdentifierValue::I8.no_prior_insignificant()),
        I16(Identifier = expect::IdentifierValue::I16.no_prior_insignificant()),
        I32(Identifier = expect::IdentifierValue::I32.no_prior_insignificant()),
        I64(Identifier = expect::IdentifierValue::I64.no_prior_insignificant()),
        Isize(Keyword = expect::Keyword::Isize.no_prior_insignificant()),
        U8(Identifier = expect::IdentifierValue::U8.no_prior_insignificant()),
        U16(Identifier = expect::IdentifierValue::U16.no_prior_insignificant()),
        U32(Identifier = expect::IdentifierValue::U32.no_prior_insignificant()),
        U64(Identifier = expect::IdentifierValue::U64.no_prior_insignificant()),
        Usize(Keyword = expect::Keyword::Usize.no_prior_insignificant()),
        F32(Identifier = expect::IdentifierValue::F32.no_prior_insignificant()),
        F64(Identifier = expect::IdentifierValue::F64.no_prior_insignificant()),
    }
}

// The fractional part of a floating-point literal, e.g. the `.5` in `1.5`. The
// dot and the digits must follow the integral part directly: `1.5` is a
// floating-point literal, while `1 . 5` and `1. 5` are not.
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
    pub struct NumericFraction {
        pub dot: Punctuation = '.'.no_prior_insignificant(),
        pub numeric: Numeric = expect::Numeric.no_prior_insignificant(),
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
    pub struct NumericLiteral {
        pub numeric: Numeric = expect::Numeric,
        pub fraction: NumericFraction = ast::<NumericFraction>().optional(),
        pub suffix: NumericSuffix = ast::<NumericSuffix>().optional()
    }
}

abstract_tree::abstract_tree! {
pub enum Leaf {
        StructInitialization(StructInitialization = ast::<StructInitialization>()),
        DirectCall(DirectCall = ast::<DirectCall>()),
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
        TupleIndex(TupleIndex = ast::<TupleIndex>()),
        FieldAccess(FieldAccess = ast::<FieldAccess>())
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
    pub struct FieldAccess {
        pub dot: Punctuation = '.',
        pub name: Identifier = expect::Identifier,
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
    pub struct IfElseExpressionArm {
        pub colon: Punctuation = ':',
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
    pub enum IfElseArm {
        Block(Block = ast::<Block>()),
        Expression(IfElseExpressionArm = ast::<IfElseExpressionArm>()),
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
    pub struct IfElseElifArm {
        pub elif_keyword: Keyword = expect::Keyword::Elif.new_line_significant(false),
        pub condition: IfElseCondition = ast::<IfElseCondition>(),
        pub arm: IfElseArm = ast::<IfElseArm>(),
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
        pub else_keyword: Keyword = expect::Keyword::Else.new_line_significant(false),
        pub arm: IfElseArm = ast::<IfElseArm>(),
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
        pub then_arm: IfElseArm = ast::<IfElseArm>(),
        pub elif_arms: #[multi] IfElseElifArm = ast::<IfElseElifArm>().repeat(),
        pub else_arm: IfElseElseArm = ast::<IfElseElseArm>().optional(),
    }
}

// The arm is either an indented block, which has no value, or `: expression`,
// whose value is the value of the whole `unsafe`.
abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Unsafe {
        pub unsafe_keyword: Keyword = expect::Keyword::Unsafe,
        pub arm: IfElseArm = ast::<IfElseArm>(),
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct While {
        pub while_keyword: Keyword = expect::Keyword::While,
        pub condition: Expression = ast::<Expression>(),
        pub block: Block = ast::<Block>(),
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
        RunWith(RunWith = ast::<RunWith>()),
        Closure(Closure = ast::<Closure>()),
        IfElse(IfElse = ast::<IfElse>()),
        While(While = ast::<While>()),
        Unsafe(Unsafe = ast::<Unsafe>()),
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
    pub struct StructFieldInitialization {
        pub name: Identifier = expect::Identifier,
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
    #{fragment = Fragment::Delimited(DelimiterKind::Brace)}
    pub struct StructFieldInitializations {
        pub fields: #[multi] StructFieldInitialization = ast::<StructFieldInitialization>()
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
    pub struct StructInitialization {
        pub path: Path = ast::<Path>(),
        pub fields: StructFieldInitializations = ast::<StructFieldInitializations>()
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
    pub struct DirectCall {
        pub path: Path = ast::<Path>(),
        pub call: Call = ast::<Call>()
    }
}
