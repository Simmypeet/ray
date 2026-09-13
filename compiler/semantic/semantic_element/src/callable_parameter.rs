//! Syntax-owned callable parameter occurrences; independent of signature
//! resolution.
use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_symbol::GlobalSymbolID;
use rayc_syntax::def::Callable;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct CallableParameter {
    owner: GlobalSymbolID,
    parameter_index: usize,
    syntax: Callable,
}
impl CallableParameter {
    #[must_use]
    pub const fn new(owner: GlobalSymbolID, parameter_index: usize, syntax: Callable) -> Self {
        Self { owner, parameter_index, syntax }
    }
    #[must_use]
    pub const fn owner(&self) -> GlobalSymbolID { self.owner }
    /// The zero-based value-parameter index within the owner, including
    /// ordinary parameters and excluding ellipses. Used to identify both
    /// generated binders for this annotation; it is not an index into the
    /// callable-only inventory.
    #[must_use]
    pub const fn occurrence(&self) -> usize { self.parameter_index }
    #[must_use]
    pub const fn syntax(&self) -> &Callable { &self.syntax }
}
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[CallableParameter]>)]
#[extend(by_val, name = get_callable_parameters)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
