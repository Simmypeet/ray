use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Ext, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Keyword, Punctuation,
    effect_row::{ConcreteEffectRow, EffectRowAnnotation},
    path::Path,
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
    pub struct Arrow {
        pub hyphen: Punctuation = '-',
        pub greater_than: Punctuation = '>'.no_prior_insignificant()
    }
}

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
        CInt(Keyword = expect::Keyword::CInt),
        CStr(Keyword = expect::Keyword::CStr),
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
    pub struct CallableReturnType {
        pub arrow: Arrow = ast::<Arrow>(),
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
    pub struct Callable {
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub parameters: CallableParameterList = ast::<CallableParameterList>(),
        pub return_type: CallableReturnType = ast::<CallableReturnType>().optional(),
        pub effect_row: EffectRowAnnotation = ast::<EffectRowAnnotation>().optional()
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
    pub struct CallableParameterList {
        pub parameters: #[multi] Type = ast::<Type>()
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
        Tuple(Tuple = ast::<Tuple>()),
        Callable(Callable = ast::<Callable>()),
        EffectRow(ConcreteEffectRow = ast::<ConcreteEffectRow>()),
        Path(Path = ast::<Path>())
    }
}
