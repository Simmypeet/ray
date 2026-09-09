use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, Keyword, Punctuation, given::GivenArguments, r#type::Type};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Bracket)}
    pub struct PathArguments {
        pub type_arguments: #[multi] Type = ast::<Type>()
            .repeat_with_separator_and_terminator(',', ';'),
        pub given_arguments: GivenArguments = ast::<GivenArguments>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct PathSegment {
        pub identifier: Identifier = expect::Identifier,
        pub arguments: PathArguments = ast::<PathArguments>().optional()
    }
}

impl PathSegment {
    /// Returns whether this segment explicitly supplies type arguments.
    #[must_use]
    pub fn has_explicit_type_arguments(&self) -> bool {
        self.arguments().is_some_and(|arguments| arguments.type_arguments().next().is_some())
    }

    /// Returns the explicitly supplied given arguments.
    #[must_use]
    pub fn supplied_given_arguments(&self) -> Option<GivenArguments> {
        self.arguments().and_then(|arguments| arguments.given_arguments())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum PathRoot {
        This(Keyword = expect::Keyword::This),
        Segment(PathSegment = ast::<PathSegment>())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct PathContinuation {
        pub dot: Punctuation = '.',
        pub segment: PathSegment = ast::<PathSegment>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Path {
        pub root: PathRoot = ast::<PathRoot>(),
        pub rest: #[multi] PathContinuation = ast::<PathContinuation>().repeat()
    }
}

impl Path {
    /// Returns ordinary segments, including an ordinary root.
    pub fn segments(&self) -> impl Iterator<Item = PathSegment> {
        let root = match self.root() {
            Some(PathRoot::Segment(segment)) => Some(segment),
            Some(PathRoot::This(_)) | None => None,
        };
        root.into_iter().chain(self.rest().filter_map(|part| part.segment()))
    }

    /// Returns the `Identifier` if the `Path` is a simple path with a single
    /// segment and no type or given arguments.
    #[must_use]
    pub fn bare_identifier(&self) -> Option<Identifier> {
        if !matches!(self.root(), Some(PathRoot::Segment(_))) {
            return None;
        }
        let mut segments = self.segments();
        let first = segments.next()?;

        // if there's next segment, then this is not a simple path
        if segments.next().is_some() {
            return None;
        }

        // if the first segment has arguments, then this is not a simple path
        if first.arguments().is_some() {
            return None;
        }

        first.identifier()
    }
}
