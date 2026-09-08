use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, given::GivenArguments, r#type::Type};

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
    /// Returns the explicitly supplied type arguments from either path syntax.
    pub fn supplied_type_arguments(&self) -> impl Iterator<Item = Type> {
        self.arguments()
            .into_iter()
            .flat_map(|arguments| arguments.type_arguments().collect::<Vec<_>>())
    }

    /// Returns whether this segment explicitly supplies type arguments.
    #[must_use]
    pub fn has_explicit_type_arguments(&self) -> bool {
        self.arguments().is_some_and(|arguments| {
            arguments.type_arguments().next().is_some() || arguments.given_arguments().is_none()
        })
    }

    /// Returns the explicitly supplied given arguments.
    #[must_use]
    pub fn supplied_given_arguments(&self) -> Option<GivenArguments> {
        self.arguments().and_then(|arguments| arguments.given_arguments())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Path {
        pub segments: #[multi] PathSegment = ast::<PathSegment>()
            .repeat_with_separator_at_least_once('.')
    }
}

impl Path {
    /// Retursn the `Identifier` if the `Path` is a simple path with a single
    /// segment and no type or given arguments.
    #[must_use]
    pub fn bare_identifier(&self) -> Option<Identifier> {
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
