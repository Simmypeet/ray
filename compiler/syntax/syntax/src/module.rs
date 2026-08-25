use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    parser::{ParserExt, ast},
};

use crate::{def::Def, effect::Effect};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum ModuleMember {
        Def(Def = ast::<Def>()),
        Effect(Effect = ast::<Effect>())
    }
}

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
        pub members: #[multi] ModuleMember = ast::<ModuleMember>().repeat_all()
    }
}
