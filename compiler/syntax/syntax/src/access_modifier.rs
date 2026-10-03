use qbice::{Decode, Encode, StableHash};
use rayc_parser::{abstract_tree, expect};

use crate::Keyword;

abstract_tree::abstract_tree! {
    /// An access modifier written in front of a declaration, such as `pub`.
    ///
    /// A declaration without an access modifier is accessible only within
    /// the module that declares it.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum AccessModifier {
        /// `pub`: the declaration is accessible from anywhere.
        Public(Keyword = expect::Keyword::Pub)
    }
}
