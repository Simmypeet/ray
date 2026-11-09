//! Module imports written at the top of a module's content.
//!
//! ```text
//! import-list-paren ::= '(' (path (',' path)*)? ')'
//! import-list-inline ::= path (',' path)*
//! import-list ::= import-list-paren | import-list-inline
//! import ::= 'import' import-list
//!
//! from-import ::= ('from' path)? import
//! ```
//!
//! Each import occupies its own line. Inside the parentheses, newlines are
//! insignificant, so a parenthesized list may span several lines.

use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{
        Ast, Choice, IntoChoice, Line, Lookahead, ParserExt, Recoverable, RepeatWithCommit, ast,
    },
};

use crate::{Keyword, path::Path};

abstract_tree::abstract_tree! {
    /// `(a, b.c, ...)`
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct ParenthesizedImportList {
        pub paths: #[multi] Path = ast::<Path>().repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    /// `a, b.c, ...`
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct InlineImportList {
        pub paths: #[multi] Path = ast::<Path>().repeat_with_separator_at_least_once(',')
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
    /// Returns the imported paths regardless of how the list is written.
    pub fn paths(&self) -> impl Iterator<Item = Path> {
        let (parenthesized, inline) = match self {
            Self::Parenthesized(list) => (Some(list.paths()), None),
            Self::Inline(list) => (None, Some(list.paths())),
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
