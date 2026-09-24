//! Attributes written on their own line above a declaration, such as
//! `@linear`.

use qbice::{Decode, Encode, StableHash};
use rayc_parser::{abstract_tree, expect};

use crate::{Identifier, Punctuation};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Attribute {
        pub at: Punctuation = '@',
        pub name: Identifier = expect::Identifier
    }
}
