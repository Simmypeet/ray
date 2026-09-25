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

// REVIEW: Should we collapse this into a simple struct, do we really need to
// differentiate between `Region` and `Type`?
/// A requirement that one lifetime, or every lifetime in a type, outlives a
/// lifetime.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Identifiable,
)]
pub enum OutlivesPredicate {
    /// `'longer: 'shorter`, where both operands have kind `Lifetime`.
    Region { longer: Interned<Ty>, shorter: Interned<Ty> },

    /// `ty: 'bound`, where `ty` has kind `Star` or `EffectRow`: every
    /// lifetime in `ty` outlives `bound`.
    Type { ty: Interned<Ty>, bound: Interned<Ty> },
}

impl OutlivesPredicate {
    /// Returns the operand that must live longer: the longer lifetime or the
    /// bounded type.
    #[must_use]
    pub const fn subject(&self) -> &Interned<Ty> {
        match self {
            Self::Region { longer, .. } => longer,
            Self::Type { ty, .. } => ty,
        }
    }

    /// Returns the lifetime that the subject must outlive.
    #[must_use]
    pub const fn bound(&self) -> &Interned<Ty> {
        match self {
            Self::Region { shorter, .. } => shorter,
            Self::Type { bound, .. } => bound,
        }
    }

    /// Renders the predicate as written in a where clause, such as `t: 'a`.
    pub async fn display(&self, engine: &TrackedEngine) -> String {
        format!("{}: {}", self.subject().display(engine).await, self.bound().display(engine).await)
    }
}

impl Substitutable for OutlivesPredicate {
    fn apply_subst(&self, subst: &Subst, engine: &TrackedEngine) -> Option<Self> {
        let subject = self.subject().apply_subst(subst, engine);
        let bound = self.bound().apply_subst(subst, engine);
        if subject.is_none() && bound.is_none() {
            return None;
        }

        let subject = subject.unwrap_or_else(|| self.subject().clone());
        let bound = bound.unwrap_or_else(|| self.bound().clone());
        Some(match self {
            Self::Region { .. } => Self::Region { longer: subject, shorter: bound },
            Self::Type { .. } => Self::Type { ty: subject, bound },
        })
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
