use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Ext, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, Keyword, Punctuation, effect_row::ConcreteEffectRow, path::Path};

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
        Int8(Keyword = expect::Keyword::Int8),
        Int16(Keyword = expect::Keyword::Int16),
        Int32(Keyword = expect::Keyword::Int32),
        Int64(Keyword = expect::Keyword::Int64),
        Isize(Keyword = expect::Keyword::Isize),
        Uint8(Keyword = expect::Keyword::Uint8),
        Uint16(Keyword = expect::Keyword::Uint16),
        Uint32(Keyword = expect::Keyword::Uint32),
        Uint64(Keyword = expect::Keyword::Uint64),
        Usize(Keyword = expect::Keyword::Usize),
        Bool(Keyword = expect::Keyword::Bool),
        Float32(Keyword = expect::Keyword::Float32),
        Float64(Keyword = expect::Keyword::Float64),
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

// The quote and the name are separate tokens, so the name must follow the
// quote directly: `'a` is a lifetime, while `' a` is not.
abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum LifetimeName {
        Static(Keyword = expect::Keyword::Static.no_prior_insignificant()),
        Identifier(Identifier = expect::Identifier.no_prior_insignificant())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Lifetime {
        pub quote: Punctuation = '\'',
        pub name: LifetimeName = ast::<LifetimeName>()
    }
}

impl Lifetime {
    /// Returns the name after the quote, such as `a` for `'a`, or `None` for
    /// `'static`.
    #[must_use]
    pub fn identifier(&self) -> Option<Identifier> {
        match self.name()? {
            LifetimeName::Static(_) => None,
            LifetimeName::Identifier(identifier) => Some(identifier),
        }
    }

    /// Returns whether this is the placeholder lifetime `'_`, which stands
    /// for an elided lifetime.
    #[must_use]
    pub fn is_placeholder(&self) -> bool {
        self.identifier().is_some_and(|identifier| &*identifier.kind.0 == "_")
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
    pub struct Reference {
        pub ampersand: Punctuation = '&',
        pub lifetime: Lifetime = ast::<Lifetime>().optional(),
        pub mut_keyword: Keyword = expect::Keyword::Mut.optional(),
        pub pointed_type: Type = ast::<Type>()
    }
}

impl Reference {
    /// Returns the written lifetime, or `None` when the lifetime is elided,
    /// either by leaving it out or by writing `'_`.
    #[must_use]
    pub fn explicit_lifetime(&self) -> Option<Lifetime> {
        self.lifetime().filter(|lifetime| !lifetime.is_placeholder())
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
        Reference(Reference = ast::<Reference>()),
        Lifetime(Lifetime = ast::<Lifetime>()),
        Tuple(Tuple = ast::<Tuple>()),
        EffectRow(ConcreteEffectRow = ast::<ConcreteEffectRow>()),
        Path(Path = ast::<Path>())
    }
}
