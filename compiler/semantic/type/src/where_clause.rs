//! Resolved predicates declared in a symbol's where clause.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::GlobalSymbolID;

use crate::{
    subst::{Subst, Substitutable},
    ty::Ty,
};

/// An equality between types, including associated type projections.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
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

impl Substitutable for AssociatedTypeEquality {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        match (self.left.apply_subst(subst, engine), self.right.apply_subst(subst, engine)) {
            (Some(left), Some(right)) => Some(Self::new(left, right)),
            (Some(left), None) => Some(Self::new(left, self.right.clone())),
            (None, Some(right)) => Some(Self::new(self.left.clone(), right)),
            (None, None) => None,
        }
    }
}

/// A requirement that a type satisfies a marker.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct MarkerPredicate {
    marker_id: GlobalSymbolID,
    implementor: Interned<Ty>,
}

impl MarkerPredicate {
    #[must_use]
    pub const fn new(marker_id: GlobalSymbolID, implementor: Interned<Ty>) -> Self {
        Self { marker_id, implementor }
    }

    #[must_use]
    pub const fn marker_id(&self) -> GlobalSymbolID { self.marker_id }

    #[must_use]
    pub const fn implementor(&self) -> &Interned<Ty> { &self.implementor }
}

impl Substitutable for MarkerPredicate {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        self.implementor
            .apply_subst(subst, engine)
            .map(|implementor| Self::new(self.marker_id, implementor))
    }
}

/// A requirement `lesser: greater`: the lifetime `lesser`, or every lifetime
/// in the type, effect row, or dictionary `lesser`, outlives the lifetime
/// `greater`.
///
/// Both `'a: 'b` and `t: 'a` share this form, because a lifetime is its own
/// only outlives component (see [`Ty::outlives_components`]).
///
/// The names follow [`TyRelate`](crate::constraint::ty_relate::TyRelate): a
/// lifetime that outlives another is its subtype, so `'lesser: 'greater`
/// holds exactly when `'lesser <: 'greater`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub struct OutlivesPredicate {
    lesser: Interned<Ty>,
    greater: Interned<Ty>,
}

impl OutlivesPredicate {
    #[must_use]
    pub const fn new(lesser: Interned<Ty>, greater: Interned<Ty>) -> Self {
        Self { lesser, greater }
    }

    /// Returns the operand that must live longer: a lifetime, a type, an
    /// effect row, or a dictionary.
    #[must_use]
    pub const fn lesser(&self) -> &Interned<Ty> { &self.lesser }

    /// Returns the lifetime that the lesser operand must outlive.
    #[must_use]
    pub const fn greater(&self) -> &Interned<Ty> { &self.greater }

    /// Renders the predicate as written in a where clause, such as `t: 'a`.
    pub async fn display(&self, engine: &TrackedEngine) -> String {
        format!("{}: {}", self.lesser.display(engine).await, self.greater.display(engine).await)
    }
}

impl Substitutable for OutlivesPredicate {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        match (self.lesser.apply_subst(subst, engine), self.greater.apply_subst(subst, engine)) {
            (None, None) => None,
            (lesser, greater) => Some(Self::new(
                lesser.unwrap_or_else(|| self.lesser.clone()),
                greater.unwrap_or_else(|| self.greater.clone()),
            )),
        }
    }
}

/// The requirement expressed by a where-clause predicate.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum PredicateKind {
    AssociatedTypeEquality(AssociatedTypeEquality),
    Marker(MarkerPredicate),
    Outlives(OutlivesPredicate),
}

impl PredicateKind {
    /// Returns the outlives predicate, if this is one.
    #[must_use]
    pub const fn as_outlives(&self) -> Option<&OutlivesPredicate> {
        match self {
            Self::Outlives(predicate) => Some(predicate),
            Self::AssociatedTypeEquality(_) | Self::Marker(_) => None,
        }
    }

    /// Returns the associated type equality, if this is one.
    #[must_use]
    pub const fn as_equality(&self) -> Option<&AssociatedTypeEquality> {
        match self {
            Self::AssociatedTypeEquality(equality) => Some(equality),
            Self::Outlives(_) | Self::Marker(_) => None,
        }
    }
}

impl Substitutable for PredicateKind {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        match self {
            Self::AssociatedTypeEquality(equality) => {
                equality.apply_subst(subst, engine).map(Self::AssociatedTypeEquality)
            }
            Self::Marker(predicate) => predicate.apply_subst(subst, engine).map(Self::Marker),
            Self::Outlives(predicate) => predicate.apply_subst(subst, engine).map(Self::Outlives),
        }
    }
}

/// Where a predicate of a [`WhereClause`] comes from.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    StableHash,
    Encode,
    Decode,
    Identifiable,
)]
pub enum PredicateOrigin {
    /// Written by the user in the where clause.
    Declared,

    /// Implied by the well-formedness of the declaration; see [`WhereClause`].
    Implied,
}

/// A resolved requirement, the source span that introduces it, and whether
/// it was written or implied.
///
/// A declared predicate's span is the predicate as written; an implied one's
/// is the declaration that implies it.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct Predicate {
    kind: PredicateKind,
    span: RelativeSpan,
    origin: PredicateOrigin,
}

impl Predicate {
    #[must_use]
    pub const fn new(kind: PredicateKind, span: RelativeSpan, origin: PredicateOrigin) -> Self {
        Self { kind, span, origin }
    }

    #[must_use]
    pub const fn kind(&self) -> &PredicateKind { &self.kind }

    #[must_use]
    pub const fn span(&self) -> RelativeSpan { self.span }

    #[must_use]
    pub const fn origin(&self) -> PredicateOrigin { self.origin }

    /// Returns whether the user wrote this predicate in the where clause.
    #[must_use]
    pub const fn is_declared(&self) -> bool {
        match self.origin {
            PredicateOrigin::Declared => true,
            PredicateOrigin::Implied => false,
        }
    }
}

/// The predicates that hold for a symbol: those declared in its where clause,
/// in declaration order, followed by the outlives bounds implied by the
/// well-formedness of its declaration.
///
/// Only plain `def`s, structs, and marker implementations have implied
/// bounds:
/// - for a `def`, the bounds implied by the references in its parameter and
///   return types;
/// - for a struct, its inferred outlives predicates;
/// - for a marker implementation, every requirement of naming its head, marker
///   predicates included.
///
/// Every other declaration spells its bounds out in its where clause.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct WhereClause {
    predicates: Interned<[Predicate]>,
}

impl WhereClause {
    #[must_use]
    pub const fn new(predicates: Interned<[Predicate]>) -> Self { Self { predicates } }

    /// Returns every predicate that holds for the symbol: the declared ones,
    /// in declaration order, followed by the implied bounds.
    #[must_use]
    pub fn predicates(&self) -> impl ExactSizeIterator<Item = &Predicate> { self.predicates.iter() }

    /// Returns the predicates written in the where clause, in declaration
    /// order.
    pub fn declared(&self) -> impl Iterator<Item = &Predicate> {
        self.predicates.iter().filter(|predicate| predicate.is_declared())
    }
}

/// Retrieves every predicate that holds for a symbol: its declared where
/// clause and its implied bounds; see [`WhereClause`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<WhereClause>)]
#[extend(by_val, name = get_where_clause)]
pub struct Key {
    pub symbol_id: GlobalSymbolID,
}

/// Retrieves only the predicates written in the where clause of a symbol
/// supporting where clauses, without implied bounds.
///
/// Prefer [`get_where_clause`]. This query exists for computing implied bounds
/// themselves, which read the declared predicates of other symbols, and for
/// the diagnostics of resolving the clause.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<WhereClause>)]
#[extend(by_val, name = get_declared_where_clause)]
pub struct DeclaredKey {
    pub symbol_id: GlobalSymbolID,
}
