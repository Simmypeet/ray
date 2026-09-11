use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, def::DefSignature, effect::TypeParameterList, given::GivenParameterList,
    kind::KindAscription, where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TraitAssociatedType {
        pub type_keyword: Keyword = expect::Keyword::Type,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub kind_ascription: KindAscription = ast::<KindAscription>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum TraitMember {
        Definition(DefSignature = ast::<DefSignature>()),
        AssociatedType(TraitAssociatedType = ast::<TraitAssociatedType>())
    }
}

impl TraitMember {
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
        pub definitions: #[multi] TraitMember = ast::<TraitMember>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Trait {
        pub trait_keyword: Keyword = expect::Keyword::Trait,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional(),
        pub body: TraitBody = ast::<TraitBody>()
    }
}
