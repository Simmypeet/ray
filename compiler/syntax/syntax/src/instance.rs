use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Punctuation, def::Def, effect::TypeParameterList,
    given::GivenParameterList, kind::KindAscription, path::Path, r#type::Type,
    where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct InstanceAssociatedType {
        pub type_keyword: Keyword = expect::Keyword::Type,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub kind_ascription: KindAscription = ast::<KindAscription>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional(),
        pub equals: Punctuation = '=',
        pub r#type: Type = ast::<Type>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{label = rayc_parser::expect::Label::InstanceMember}
    pub enum InstanceMember {
        Definition(Def = ast::<Def>()),
        AssociatedType(InstanceAssociatedType = ast::<InstanceAssociatedType>())
    }
}

impl InstanceMember {
    #[must_use]
    pub fn definition(&self) -> Option<Def> {
        match self {
            Self::Definition(definition) => Some(definition.clone()),
            Self::AssociatedType(_) => None,
        }
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct InstanceBody {
        pub definitions: #[multi] InstanceMember = ast::<InstanceMember>().line().repeat_all()
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
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional(),
        pub body: InstanceBody = ast::<InstanceBody>()
    }
}
