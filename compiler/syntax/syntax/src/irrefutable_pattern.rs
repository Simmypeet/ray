use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree, expect,
    parser::{ParserExt, ast},
};

use crate::{Identifier, Keyword};

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
    pub struct Name {
        pub mut_keyword: Keyword = expect::Keyword::Mut.optional(),
        pub identifier: Identifier = expect::Identifier,
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
    pub enum IrrefutablePattern {
        Name(Name = ast::<Name>()),
    }
}
