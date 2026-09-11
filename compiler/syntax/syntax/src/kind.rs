//! Kind names and declaration ascriptions. Kind names are contextual
//! identifiers.

use qbice::{Decode, Encode, StableHash};
use rayc_parser::{abstract_tree, expect, parser::ast};

use crate::{Identifier, Punctuation};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum Kind {
        Star(Identifier = expect::IdentifierValue::Star),
        Effect(Identifier = expect::IdentifierValue::Effect)
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct KindAscription {
        pub colon: Punctuation = ':',
        pub kind: Kind = ast::<Kind>()
    }
}
