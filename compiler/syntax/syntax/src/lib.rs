#![allow(missing_docs)]

//! Contains all the definitions of the syntax tree

use std::path::Path;

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_lexical::{kind as lexical_kind, token::Token, tree::RelativeLocation};
use rayc_parser::{
    abstract_tree::{self, AbstractTree},
    expect,
    parser::ast,
};
use rayc_target::TargetID;

use crate::module::ModuleContent;

pub mod access_modifier;
pub mod attribute;
pub mod def;
pub mod effect;
pub mod effect_row;
pub mod expression;
pub mod extern_def;
pub mod given;
pub mod instance;
pub mod irrefutable_pattern;
pub mod kind;
pub mod marker;
pub mod module;
pub mod path;
pub mod statement;
pub mod r#struct;
pub mod r#trait;
pub mod r#type;
pub mod where_clause;

/// Type alias for [`Token`] categorized as a [`lexical_kind::Keyword`].
pub type Keyword = Token<lexical_kind::Keyword, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::NewLine`].
pub type NewLine = Token<lexical_kind::NewLine, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::Character`].
pub type Character = Token<lexical_kind::Character, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::String`].
pub type String = Token<lexical_kind::String, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::Identifier`].
pub type Identifier = Token<lexical_kind::Identifier, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::Punctuation`].
pub type Punctuation = Token<lexical_kind::Punctuation, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`lexical_kind::Numeric`].
pub type Numeric = Token<lexical_kind::Numeric, RelativeLocation>;

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
    pub enum Passable<T: AbstractTree> {
        Ast(T = ast::<T>()),
        Pass(Keyword = expect::Keyword::Pass)
    }
}

impl<T: AbstractTree> Passable<T> {
    /// Returns the inner syntax tree, or [`None`] if this is a `pass`.
    #[must_use]
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Ast(ast) => Some(ast),
            Self::Pass(_) => None,
        }
    }
}

/// Query for parsing a token tree from the given source file path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query)]
#[value(Result<
    (Option<ModuleContent>, Interned<[rayc_parser::error::Error]>),
    rayc_source_file::Error
>)]
pub struct Key {
    /// The path to load the source file.
    pub path: Interned<Path>,

    /// The target ID that requested the source file parsing.
    pub target_id: TargetID,
}

/// A key for retrieving the diagnostics that occurred while parsing the token
/// tree from the source file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query)]
#[value(Result<Interned<[rayc_parser::error::Error]>, rayc_source_file::Error>)]
pub struct DiagnosticKey(pub Key);
