use qbice::{Decode, Encode, StableHash};
use rayc_lexical::tree::DelimiterKind;
use rayc_parser::{
    abstract_tree,
    expect::{self, Fragment},
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, Punctuation,
    def::{ParameterList, ReturnType},
    given::GivenParameterList,
    kind::KindAscription,
    r#type::Lifetime,
    where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TypeVariableParameter {
        pub name: Identifier = expect::Identifier,
        pub kind_ascription: KindAscription = ast::<KindAscription>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum TypeParameterKind {
        Lifetime(Lifetime = ast::<Lifetime>()),
        Variable(TypeVariableParameter = ast::<TypeVariableParameter>())
    }
}

abstract_tree::abstract_tree! {
    /// The variance written before a type parameter: `+` for covariant, `-`
    /// for contravariant, and `=` for invariant.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub enum VarianceMarker {
        Covariant(Punctuation = '+'),
        Contravariant(Punctuation = '-'),
        Invariant(Punctuation = '=')
    }
}

abstract_tree::abstract_tree! {
    /// A type or lifetime parameter, optionally preceded by its variance, as
    /// in `+'a` or `=t`.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct TypeParameter {
        pub variance: VarianceMarker = ast::<VarianceMarker>().optional(),
        pub kind: TypeParameterKind = ast::<TypeParameterKind>()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Delimited(DelimiterKind::Bracket)}
    pub struct TypeParameterList {
        pub parameters: #[multi] TypeParameter = ast::<TypeParameter>()
            .repeat_all_with_separator(',')
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct OperationSignature {
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub name: Identifier = expect::Identifier,
        pub parameter_list: ParameterList = ast::<ParameterList>(),
        pub return_type: ReturnType = ast::<ReturnType>().optional()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    #{fragment = Fragment::Indentation}
    pub struct EffectBody {
        pub operation_signatures: #[multi] OperationSignature
            = ast::<OperationSignature>().line().repeat_all()
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Effect {
        pub eff_keyword: Keyword = expect::Keyword::Eff,
        pub name: Identifier = expect::Identifier,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub given_parameter_list: GivenParameterList = ast::<GivenParameterList>().optional(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional(),
        pub body: EffectBody = ast::<EffectBody>()
    }
}
