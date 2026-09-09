//! Resolved type equalities declared in a symbol's where clause.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;
use rayc_type::ty::Ty;

/// An equality between types, including associated type projections.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct AssociatedTypeEquality {
    left: Interned<Ty>,
    right: Interned<Ty>,
    span: RelativeSpan,
}

impl AssociatedTypeEquality {
    #[must_use]
    pub const fn new(left: Interned<Ty>, right: Interned<Ty>, span: RelativeSpan) -> Self {
        Self { left, right, span }
    }

    #[must_use]
    pub const fn left(&self) -> &Interned<Ty> { &self.left }

    #[must_use]
    pub const fn right(&self) -> &Interned<Ty> { &self.right }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// Equalities in declaration order. An absent clause produces an empty list.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct WhereClause {
    equalities: Interned<[AssociatedTypeEquality]>,
}

impl WhereClause {
    #[must_use]
    pub const fn new(equalities: Interned<[AssociatedTypeEquality]>) -> Self { Self { equalities } }

    #[must_use]
    pub fn len(&self) -> usize { self.equalities.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.equalities.is_empty() }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &AssociatedTypeEquality> {
        self.equalities.iter()
    }
}

/// Retrieves the resolved where clause for a symbol supporting where clauses.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<WhereClause>)]
#[extend(by_val, name = get_where_clause)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}
