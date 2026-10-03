use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Passable, Punctuation, access_modifier::AccessModifier,
    attribute::Attribute, effect::TypeParameterList, given::GivenParameterList, r#type::Type,
    where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct StructField {
        pub access_modifier: AccessModifier = ast::<AccessModifier>().optional(),
        pub name: Identifier = expect::Identifier,
        pub colon: Punctuation = ':',
        pub r#type: Type = ast::<Type>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct StructBody {
        pub fields: #[multi] Passable<StructField> =
            ast::<Passable<StructField>>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Struct {
        pub attributes: #[multi] Attribute = ast::<Attribute>().line().repeat(),
        pub access_modifier: AccessModifier = ast::<AccessModifier>().optional(),
        pub struct_keyword: Keyword = expect::Keyword::Struct,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional(),
        pub body: StructBody = ast::<StructBody>().optional()
    }
}
