//! Module imports written at the top of a module's content.
//!
//! ```text
//! import-item ::= path ('as' identifier)?
//! import-list-paren ::= '(' (import-item (',' import-item)*)? ')'
//! import-list-inline ::= import-item (',' import-item)*
//! import-list ::= import-list-paren | import-list-inline
//! import ::= 'import' import-list
//!
//! from-import ::= ('from' path)? import
//! ```
//!
//! Each import occupies its own line. Inside the parentheses, newlines are
//! insignificant, so a parenthesized list may span several lines.

use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::{DelimiterKind, RelativeSpan};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{
        Ast, Choice, IntoChoice, Line, Lookahead, ParserExt, Recoverable, RepeatWithCommit, ast,
    },
};
use rayc_source_file::SourceElement;

use crate::{
    Identifier, Keyword,
    path::{Path, PathRoot},
};

abstract_tree::abstract_tree! {
    /// `as name`, the name that an imported path is brought in under.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct ImportAlias {
        pub as_keyword: Keyword = expect::Keyword::As,
        pub name: Identifier = expect::Identifier
    }
}

abstract_tree::abstract_tree! {
    /// `path` or `path as name`.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct ImportItem {
        pub path: Path = ast::<Path>(),
        // committing on `as` reports a missing name after it, instead of
        // leaving the `as` behind as an unexpected token
        pub alias: ImportAlias =
            ast::<ImportAlias>().commit_if(expect::Keyword::As.lookahead())
    }
}

/// The name that an [`ImportItem`] binds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportItemName {
    /// The alias of the item, or else the last segment of its path.
    Identifier(Identifier),

    /// The path consists of only a keyword root, such as `super`, which can't
    /// be used as a name; the span is the one of the path.
    Keyword(RelativeSpan),

    /// The name is missing because of a syntax error, which the parser has
    /// reported.
    Missing,
}

impl ImportItem {
    /// Returns the name that this item binds: its alias, or else the last
    /// segment of its path.
    #[must_use]
    pub fn bound_name(&self) -> ImportItemName {
        if let Some(alias) = self.alias() {
            return alias.name().map_or(ImportItemName::Missing, ImportItemName::Identifier);
        }
        let Some(path) = self.path() else { return ImportItemName::Missing };

        // the last segment written after the root, if any
        if let Some(last) = path.rest().last() {
            return last
                .segment()
                .and_then(|segment| segment.identifier())
                .map_or(ImportItemName::Missing, ImportItemName::Identifier);
        }

        match path.root() {
            Some(PathRoot::Segment(segment)) => {
                segment.identifier().map_or(ImportItemName::Missing, ImportItemName::Identifier)
            }
            Some(PathRoot::This(_) | PathRoot::Target(_) | PathRoot::Super(_)) => {
                ImportItemName::Keyword(path.span())
            }
            None => ImportItemName::Missing,
        }
    }
}

abstract_tree::abstract_tree! {
    /// `(a, b.c as d, ...)`
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct ParenthesizedImportList {
        pub items: #[multi] ImportItem = ast::<ImportItem>().repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    /// `a, b.c as d, ...`
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct InlineImportList {
        pub items: #[multi] ImportItem =
            ast::<ImportItem>().repeat_with_separator_at_least_once(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum ImportList {
        Parenthesized(ParenthesizedImportList = ast::<ParenthesizedImportList>()),
        Inline(InlineImportList = ast::<InlineImportList>())
    }
}

impl ImportList {
    /// Returns the imported items regardless of how the list is written.
    pub fn items(&self) -> impl Iterator<Item = ImportItem> {
        let (parenthesized, inline) = match self {
            Self::Parenthesized(list) => (Some(list.items()), None),
            Self::Inline(list) => (None, Some(list.items())),
        };

        parenthesized.into_iter().flatten().chain(inline.into_iter().flatten())
    }
}

abstract_tree::abstract_tree! {
    /// `from path`, the module that the imported paths are relative to.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct FromClause {
        pub from_keyword: Keyword = expect::Keyword::From,
        pub path: Path = ast::<Path>()
    }
}

abstract_tree::abstract_tree! {
    /// `from path import list` or `import list`.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Import {
        pub from: FromClause = ast::<FromClause>().optional(),
        pub import_keyword: Keyword = expect::Keyword::Import,
        pub list: ImportList = ast::<ImportList>()
    }
}

/// The parser of the imports written at the top of a module's content.
pub(crate) type ImportLines = RepeatWithCommit<
    Lookahead<Choice<(expect::Keyword, expect::Keyword)>>,
    Recoverable<Line<Ast<Import>>>,
>;

impl Import {
    /// Parses the imports at the top of a module's content, one per line.
    ///
    /// A line starting with `from` or `import` commits to an import, so a
    /// malformed import is reported as such and parsing continues with the
    /// next line instead of reading the line as a module member.
    pub(crate) fn lines() -> ImportLines {
        ast::<Self>().line().recoverable().repeat_with_commit(
            (expect::Keyword::From, expect::Keyword::Import).into_choice().lookahead(),
        )
    }
}
