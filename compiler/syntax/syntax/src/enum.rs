use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Passable, access_modifier::AccessModifier, attribute::Attribute,
    effect::TypeParameterList, given::GivenParameterList, r#type::Type, where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    /// The parenthesized type carried by a variant, such as `(int32)` in
    /// `Some(int32)`.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Parenthesis)}
    pub struct VariantPayload {
        pub r#type: Type = ast::<Type>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Variant {
        pub name: Identifier = expect::Identifier,
        pub payload: VariantPayload = ast::<VariantPayload>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct EnumBody {
        pub variants: #[multi] Passable<Variant> =
            ast::<Passable<Variant>>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct EnumSignature {
        pub enum_keyword: Keyword = expect::Keyword::Enum,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional()
    }
}

abstract_tree::abstract_tree! {
    /// An enum declaration. A declaration without a body (`enum Never`)
    /// declares an enum with no variants.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Enum {
        pub attributes: #[multi] Attribute = ast::<Attribute>().line().repeat(),
        pub access_modifier: AccessModifier = ast::<AccessModifier>().optional(),
        pub signature: EnumSignature = ast::<EnumSignature>(),
        pub body: EnumBody = ast::<EnumBody>().optional()
    }
}
