use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    parser::{ParserExt, ast},
};

use crate::def::Def;

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
    pub struct ModuleContent {
        pub defs: #[multi] Def = ast::<Def>().repeat_all()
    }
}
