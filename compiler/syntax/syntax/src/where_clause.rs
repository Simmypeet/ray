//! Declaration constraints written as `where (left = right, T: Marker, ...)`.
//!
//! The keyword and opening parenthesis stay on the declaration header's line.
//! Inside the parentheses, newlines are insignificant and commas separate
//! constraints, including an optional trailing comma. Equality operands and
//! marker implementors are parsed as types; resolving and enforcing each
//! predicate belongs to semantic analysis.

use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Keyword, Punctuation, path::Path, r#type::Type};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct MarkerPredicate {
        pub implementor: Type = ast::<Type>(),
        pub colon: Punctuation = ':',
        pub marker: Path = ast::<Path>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TypeEquality {
        pub left: Type = ast::<Type>(),
        pub equals: Punctuation = '=',
        pub right_operand: EqualityRightOperand = ast::<EqualityRightOperand>()
    }
}

// Field extraction selects the first child of a matching syntax type. Wrap
// the right operand so it remains distinct from the left operand.
abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct EqualityRightOperand {
        pub r#type: Type = ast::<Type>()
    }
}

impl TypeEquality {
    #[must_use]
    pub fn right(&self) -> Option<Type> {
        self.right_operand().and_then(|operand| operand.r#type())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum Constraint {
        TypeEquality(TypeEquality = ast::<TypeEquality>()),
        MarkerPredicate(MarkerPredicate = ast::<MarkerPredicate>())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct Constraints {
        pub constraints: #[multi] Constraint = ast::<Constraint>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct WhereClause {
        pub where_keyword: Keyword = expect::Keyword::Where.new_line_significant(true),
        pub constraints: Constraints = ast::<Constraints>().new_line_significant(true)
    }
}
