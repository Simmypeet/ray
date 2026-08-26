use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, Punctuation, path::Path};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct EffectRowTail {
        pub pipe: Punctuation = '|',
        pub variable: Identifier = expect::Identifier
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Brace)}
    pub struct EffectRow {
        pub effects: #[multi] Path = ast::<Path>().repeat_with_separator(','),
        pub tail: EffectRowTail = ast::<EffectRowTail>().optional()
    }
}
