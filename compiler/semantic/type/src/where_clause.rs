//! Resolved predicates declared in a symbol's where clause.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_symbol::GlobalSymbolID;

use crate::ty::Ty;

/// An equality between types, including associated type projections.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct AssociatedTypeEquality {
    left: Interned<Ty>,
    right: Interned<Ty>,
}

impl AssociatedTypeEquality {
    #[must_use]
    pub const fn new(left: Interned<Ty>, right: Interned<Ty>) -> Self { Self { left, right } }

    #[must_use]
    pub const fn left(&self) -> &Interned<Ty> { &self.left }

    #[must_use]
    pub const fn right(&self) -> &Interned<Ty> { &self.right }
}

/// The requirement expressed by a where-clause predicate.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub enum PredicateKind {
    AssociatedTypeEquality(AssociatedTypeEquality),
}

/// A resolved requirement and the source span where it was declared.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct Predicate {
    kind: PredicateKind,
    span: RelativeSpan,
}

impl Predicate {
    #[must_use]
    pub const fn new(kind: PredicateKind, span: RelativeSpan) -> Self { Self { kind, span } }

    #[must_use]
    pub const fn kind(&self) -> &PredicateKind { &self.kind }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }
}

/// Predicates in declaration order. An absent clause produces an empty list.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct WhereClause {
    predicates: Interned<[Predicate]>,
}

impl WhereClause {
    #[must_use]
    pub const fn new(predicates: Interned<[Predicate]>) -> Self { Self { predicates } }

    #[must_use]
    pub fn len(&self) -> usize { self.predicates.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.predicates.is_empty() }

    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Predicate> { self.predicates.iter() }
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
