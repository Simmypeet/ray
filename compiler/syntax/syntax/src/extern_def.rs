use qbice::{Decode, Encode, StableHash};
use rayc_parser::{
    abstract_tree, expect,
    parser::{ParserExt, ast},
};

use crate::{
    Identifier, Keyword,
    def::{ParameterList, ReturnType},
};

abstract_tree::abstract_tree! {
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
    pub struct ExternDef {
        pub extern_keyword: Keyword = expect::Keyword::Extern,
        pub def_keyword: Keyword = expect::Keyword::Def,
        pub name: Identifier = expect::Identifier,
        pub parameter_list: ParameterList = ast::<ParameterList>(),
        pub return_type: ReturnType = ast::<ReturnType>().optional()
    }
}
