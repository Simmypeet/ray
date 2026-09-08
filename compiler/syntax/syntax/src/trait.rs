use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, def::DefSignature, effect::TypeParameterList, given::GivenParameterList,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TraitAssociatedType {
        pub type_keyword: Keyword = expect::Keyword::Type,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum TraitDef {
        Definition(DefSignature = ast::<DefSignature>()),
        AssociatedType(TraitAssociatedType = ast::<TraitAssociatedType>())
    }
}

impl TraitDef {
    #[must_use]
    pub fn signature(&self) -> Option<DefSignature> {
        match self {
            Self::Definition(signature) => Some(signature.clone()),
            Self::AssociatedType(_) => None,
        }
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct TraitBody {
        pub definitions: #[multi] TraitDef = ast::<TraitDef>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Trait {
        pub trait_keyword: Keyword = expect::Keyword::Trait,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub body: TraitBody = ast::<TraitBody>()
    }
}
