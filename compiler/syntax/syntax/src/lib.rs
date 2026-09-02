#![allow(missing_docs)]

//! Contains all the definitions of the syntax tree

use std::path::Path;

use qbice::{Decode, Encode, Query, StableHash, storage::intern::Interned};
use rayc_lexical::{kind, token::Token, tree::RelativeLocation};
use rayc_target::TargetID;

use crate::module::ModuleContent;

pub mod def;
pub mod effect;
pub mod effect_row;
pub mod expression;
pub mod irrefutable_pattern;
pub mod module;
pub mod path;
pub mod statement;
pub mod r#trait;
pub mod r#type;

/// Type alias for [`Token`] categorized as a [`kind::Keyword`].
pub type Keyword = Token<kind::Keyword, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::NewLine`].
pub type NewLine = Token<kind::NewLine, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::Character`].
pub type Character = Token<kind::Character, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::String`].
pub type String = Token<kind::String, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::Identifier`].
pub type Identifier = Token<kind::Identifier, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::Punctuation`].
pub type Punctuation = Token<kind::Punctuation, RelativeLocation>;

/// Type alias for [`Token`] categorized as a [`kind::Numeric`].
pub type Numeric = Token<kind::Numeric, RelativeLocation>;

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
