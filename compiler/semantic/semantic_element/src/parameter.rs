use bon::Builder;
use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

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
    Decode,
    Identifiable,
    Builder,
)]
pub struct Parameter {
    span: Option<RelativeSpan>,
    ty: Interned<Ty>,
}

impl Parameter {
    #[must_use]
    pub const fn ty(&self) -> &Interned<Ty> { &self.ty }
}

pub type ParameterID = ID<Parameter>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct ParameterMap {
    parameters: OrderedArena<Parameter>,
}

impl ParameterMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn len(&self) -> usize { self.parameters.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.parameters.is_empty() }

    pub fn iter(&self) -> impl Iterator<Item = (ID<Parameter>, &Parameter)> {
        self.parameters.iter()
    }

    pub fn push(&mut self, parameter: Parameter) -> ID<Parameter> {
        self.parameters.insert(parameter)
    }
}

/// Retrieves the parameters of a function symbol
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<ParameterMap>)]
#[extend(by_val, name = get_parameter_map)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
