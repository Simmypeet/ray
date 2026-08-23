use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_arena::{ID, OrderedArena};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::{GlobalSymbolID, MemberID};

use crate::ty::TyKind;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct PolyVar {
    name: Interned<str>,
    kind: TyKind,
    span: RelativeSpan,
}

impl PolyVar {
    #[must_use]
    pub const fn new(name: Interned<str>, kind: TyKind, span: RelativeSpan) -> Self {
        Self { name, kind, span }
    }

    #[must_use]
    pub fn name(&self) -> &str { &self.name }

    #[must_use]
    pub const fn kind(&self) -> TyKind { self.kind }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

pub type PolyVarID = ID<PolyVar>;
pub type GlobalPolyVarID = MemberID<PolyVarID>;

#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Default, Identifiable)]
pub struct PolyVarMap {
    poly_vars: OrderedArena<PolyVar>,
}

impl PolyVarMap {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    #[must_use]
    pub fn len(&self) -> usize { self.poly_vars.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.poly_vars.is_empty() }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (PolyVarID, &PolyVar)> {
        self.poly_vars.iter()
    }

    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<PolyVarID> {
        self.poly_vars.iter().find_map(|(id, poly_var)| (&*poly_var.name == name).then_some(id))
    }

    #[must_use]
    pub fn name_of(&self, id: PolyVarID) -> &str {
        self.poly_vars.get(id).expect("polymorphic variable ID should be valid").name()
    }

    pub fn insert(&mut self, poly_var: PolyVar) -> PolyVarID {
        if let Some(id) = self.find_by_name(&poly_var.name) {
            return id;
        }

        self.poly_vars.insert(poly_var)
    }
}

/// Retrieves the polymorphic variables declared by a function symbol.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<PolyVarMap>)]
#[extend(by_val, name = get_poly_var_map)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}

