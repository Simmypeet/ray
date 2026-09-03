use qbice::{Decode, Encode, StableHash};
use rayc_lexical::{kind, tree::DelimiterKind};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{Parser, ParserExt, Unexpected, ast},
    state::State,
};
use rayc_qbice::Interner;

use crate::{Identifier, given::GivenArgumentList, r#type::Type};

#[derive(Debug, Clone, Copy)]
struct StartsGivenArgumentList;

impl<I: Interner> Parser<I> for StartsGivenArgumentList {
    fn parse(&self, state: &mut State<'_, '_, I>) -> Result<(), Unexpected> {
        let Some((given, given_index)) = state.peek() else {
            return Err(Unexpected);
        };
        if !given.as_leaf().is_some_and(|token| {
            token.kind.as_keyword().is_some_and(|keyword| *keyword == kind::Keyword::Given)
        }) {
            return Err(Unexpected);
        }

        let Some(arguments_id) = state.branch().nodes.get(given_index + 1).and_then(|node| {
            node.as_branch().copied().filter(|id| {
                state.tree()[*id]
                    .kind
                    .as_fragment()
                    .and_then(|fragment| fragment.fragment_kind.as_delimiter())
                    .is_some_and(|delimiter| delimiter.delimiter == DelimiterKind::Parenthesis)
            })
        }) else {
            return Err(Unexpected);
        };
        let arguments = &state.tree()[arguments_id];

        let Some(first) = arguments.nodes.first().and_then(|node| node.as_leaf()) else {
            return Err(Unexpected);
        };
        if !first.kind.is_identifier() {
            return Err(Unexpected);
        }

        let starts_parameter =
            arguments.nodes.get(1).and_then(|node| node.as_leaf()).is_some_and(|token| {
                token.kind.as_punctuation().is_some_and(|punctuation| punctuation.0 == ':')
            });

        (!starts_parameter).then_some(()).ok_or(Unexpected)
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Bracket)}
    pub struct TypeArgumentList {
        pub arguments: #[multi] Type = ast::<Type>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct PathSegment {
        pub identifier: Identifier = expect::Identifier,
        pub type_arguments: TypeArgumentList = ast::<TypeArgumentList>().optional(),
        pub given_arguments: GivenArgumentList = ast::<GivenArgumentList>()
            .commit_if(StartsGivenArgumentList)
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
        if first.type_arguments().is_some() || first.given_arguments().is_some() {
            return None;
        }

        first.identifier()
    }
}
