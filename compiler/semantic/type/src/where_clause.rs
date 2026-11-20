//! Resolved predicates declared in a symbol's where clause.

use qbice::{Decode, Encode, Identifiable, Query, StableHash, storage::intern::Interned};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_symbol::{GlobalSymbolID, name::get_qualified_name};

use crate::{
    fold::{TyFoldable, TyFolder, fold_pair},
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

impl TyFoldable for AssociatedTypeEquality {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        fold_pair(&self.left, &self.right, folder, engine)
            .await
            .map(|(left, right)| Self::new(left, right))
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

impl TyFoldable for MarkerPredicate {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        self.implementor
            .fold_with(folder, engine)
            .await
            .map(|implementor| Self::new(self.marker_id, implementor))
    }
}

/// A requirement `lesser: greater`, shared by `'a: 'b` and `t: 'a` since a
/// lifetime is its own only outlives component.
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

impl TyFoldable for OutlivesPredicate {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        fold_pair(&self.lesser, &self.greater, folder, engine)
            .await
            .map(|(lesser, greater)| Self::new(lesser, greater))
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

    /// Describes what the predicate requires, as in "`t` must equal `u`".
    pub async fn describe_requirement(&self, engine: &TrackedEngine) -> String {
        match self {
            Self::AssociatedTypeEquality(equality) => {
                let left = equality.left().display(engine).await;
                let right = equality.right().display(engine).await;
                format!("`{left}` must equal `{right}`")
            }
            Self::Marker(predicate) => {
                let implementor = predicate.implementor().display(engine).await;
                let marker = engine.get_qualified_name(predicate.marker_id()).await;
                format!("`{implementor}` must satisfy marker `{marker}`")
            }
            Self::Outlives(predicate) => {
                let subject = predicate.lesser().display(engine).await;
                let bound = predicate.greater().display(engine).await;
                format!("`{subject}` must outlive `{bound}`")
            }
        }
    }
}

impl TyFoldable for PredicateKind {
    async fn fold_with(&self, folder: &mut impl TyFolder, engine: &TrackedEngine) -> Option<Self> {
        match self {
            Self::AssociatedTypeEquality(equality) => {
                equality.fold_with(folder, engine).await.map(Self::AssociatedTypeEquality)
            }
            Self::Marker(predicate) => predicate.fold_with(folder, engine).await.map(Self::Marker),
            Self::Outlives(predicate) => {
                predicate.fold_with(folder, engine).await.map(Self::Outlives)
            }
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

/// A resolved requirement with the span that introduces it, and whether it
/// was written or implied.
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

/// The predicates that hold for a symbol: its declared where clause followed
/// by the outlives bounds implied by its declaration's well-formedness.
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

/// Retrieves only the written predicates of a symbol, for computing implied
/// bounds without cycles; prefer [`get_where_clause`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<WhereClause>)]
#[extend(by_val, name = get_declared_where_clause)]
pub struct DeclaredKey {
    pub symbol_id: GlobalSymbolID,
}
