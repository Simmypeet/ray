use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::Fragment,
    parser::{ParserExt, ast},
};

use crate::{Punctuation, path::Path};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct EffectRowTail {
        pub pipe: Punctuation = '|',
        pub variable: Path = ast::<Path>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Brace)}
    pub struct ConcreteEffectRow {
        pub effects: #[multi] Path = ast::<Path>().repeat_with_separator(','),
        pub tail: EffectRowTail = ast::<EffectRowTail>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum EffectRow {
        Path(Path = ast::<Path>()),
        ConcreteEffectRow(ConcreteEffectRow = ast::<ConcreteEffectRow>())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct EffectRowAnnotation {
        pub backslash: Punctuation = '\\',
        pub effect_row: EffectRow = ast::<EffectRow>()
    }
}
