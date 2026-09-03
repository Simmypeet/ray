use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{Identifier, Keyword, def::Def, effect::TypeParameterList, path::Path};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct InstanceDef {
        pub definition: Def = ast::<Def>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct InstanceBody {
        pub definitions: #[multi] InstanceDef = ast::<InstanceDef>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Instance {
        pub inst_keyword: Keyword = expect::Keyword::Inst,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub for_keyword: Keyword = expect::Keyword::For,
        pub trait_reference: Path = ast::<Path>(),
        pub body: InstanceBody = ast::<InstanceBody>()
    }
}
