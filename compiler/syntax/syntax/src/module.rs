use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Passable,
    access_modifier::AccessModifier,
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
        MarkerImplementation(MarkerImplementation = ast::<MarkerImplementation>()),
        Module(Module = ast::<Module>())
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct ModuleBody {
        pub members: #[multi] Passable<ModuleMember> =
            ast::<Passable<ModuleMember>>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    /// A submodule declaration.
    ///
    /// A declaration with a body (`module name:`) defines its members inline.
    /// A declaration without one (`module name`) loads its members from the
    /// file `name.ray` in the directory owned by the enclosing module.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Module {
        pub access_modifier: AccessModifier = ast::<AccessModifier>().optional(),
        pub module_keyword: Keyword = expect::Keyword::Module,
        pub name: Identifier = expect::Identifier,
        pub body: ModuleBody = ast::<ModuleBody>().optional()
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
