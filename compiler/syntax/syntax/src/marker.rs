use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree, expect,
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword, effect::TypeParameterList, path::Path, r#type::Type,
    where_clause::WhereClause,
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct Marker {
        pub marker_keyword: Keyword = expect::Keyword::Marker,
        pub name: Identifier = expect::Identifier
    }
}

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct MarkerImplementation {
        pub impl_keyword: Keyword = expect::Keyword::Impl,
        pub type_parameters: TypeParameterList = ast::<TypeParameterList>().optional(),
        pub marker: Path = ast::<Path>(),
        pub for_keyword: Keyword = expect::Keyword::For,
        pub implementor: Type = ast::<Type>(),
        pub where_clause: WhereClause = ast::<WhereClause>().optional()
    }
}
