use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Keyword, Punctuation};

abstract_tree::abstract_tree! {
    #[derive(
        Debug,
        Clone,
        Copy,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        StableHash,
        Encode,
        Decode
    )]
    pub enum Primitive {
        Int32(Keyword = expect::Keyword::Int32),
        Bool(Keyword = expect::Keyword::Bool),
        Float32(Keyword = expect::Keyword::Float32),
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
    pub struct Pointer {
        pub asterisk: Punctuation = '*',
        pub mut_keyword: Keyword = expect::Keyword::Mut.optional(),
        pub pointed_type: Type = ast::<Type>()
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
    pub struct Tuple {
        pub elements: #[multi] Type = ast::<Type>()
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
    pub enum Type {
        Primitive(Primitive = ast::<Primitive>()),
        Pointer(Pointer = ast::<Pointer>()),
        Tuple(Tuple = ast::<Tuple>())
    }
}

#[cfg(test)]
mod test;
