use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    parser::{ParserExt, ast},
};

use crate::{
    Passable,
    def::Def,
    effect::Effect,
    extern_def::ExternDef,
    instance::Instance,
    marker::{Marker, MarkerImplementation},
    r#struct::Struct,
    r#trait::Trait,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum ModuleMember {
        Def(Def = ast::<Def>()),
        ExternDef(ExternDef = ast::<ExternDef>()),
        Effect(Effect = ast::<Effect>()),
        Trait(Trait = ast::<Trait>()),
        Instance(Instance = ast::<Instance>()),
        Struct(Struct = ast::<Struct>()),
        Marker(Marker = ast::<Marker>()),
        MarkerImplementation(MarkerImplementation = ast::<MarkerImplementation>())
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
        pub members: #[multi] Passable<ModuleMember> =
            ast::<Passable<ModuleMember>>().line().repeat_all()
    }
}
