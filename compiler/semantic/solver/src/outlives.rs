//! Outlives requirements between named lifetimes.
//!
//! This module answers questions such as `'a: 'b` or `t: 'a` against a fixed
//! set of known facts, without any region inference. The facts are the
//! outlives predicates declared in where clauses and the bounds implied by the
//! well-formedness of a declaration's signature.

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Identifiable, Query, StableHash, executor, program::Registration,
    storage::intern::Interned,
};
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_symbol::GlobalSymbolID;
use rayc_transitive_closure::TransitiveClosure;
use rayc_type::{
    constraint::outlives::OutlivesConstraints,
    outlives::OutlivesComponent,
    ty::{Ty, lifetime::Lifetime},
    where_clause::{OutlivesPredicate, PredicateKind},
};

use crate::givens::get_givens;

pub mod implied;

/// Retrieves the outlives facts known at a site: the outlives predicates
/// among its givens, implied bounds included.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<OutlivesEnvironment>)]
#[extend(by_val, name = get_outlives_environment)]
pub struct OutlivesEnvironmentKey {
    pub site: GlobalSymbolID,
}

#[executor(config = Config)]
pub async fn outlives_environment_executor(
    &OutlivesEnvironmentKey { site }: &OutlivesEnvironmentKey,
    engine: &TrackedEngine,
) -> Interned<OutlivesEnvironment> {
    let givens = engine.get_givens(site).await;
    let facts = outlives_givens(&givens).cloned();
    engine.intern(OutlivesEnvironment::new(facts, engine).await)
}

#[distributed_slice(RAY_PROGRAM)]
static OUTLIVES_ENVIRONMENT_EXECUTOR: Registration<Config> =
    Registration::new::<OutlivesEnvironmentKey, OutlivesEnvironmentExecutor>();

/// The outlives facts known at a declaration site, and the entailment of
/// outlives predicates from them.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode, Identifiable)]
pub struct OutlivesEnvironment {
    /// The index in `closure` of every lifetime in a region fact. `'static`
    /// is always at index zero.
    lifetimes: FxHashMap<Interned<Ty>, usize>,

    /// `closure.has_path(a, b)` means that the lifetime at index `a` is known
    /// to outlive the lifetime at index `b`.
    closure: TransitiveClosure,

    /// Known `subject: bound` facts whose subject is a type or effect-row
    /// variable, or a projection.
    type_facts: Vec<OutlivesPredicate>,
}

impl OutlivesEnvironment {
    /// Builds the environment from outlives facts. Each fact is decomposed
    /// into its components first, so `&'b int32: 'a` becomes `'b: 'a`.
    pub async fn new(
        facts: impl IntoIterator<Item = OutlivesPredicate>,
        engine: &TrackedEngine,
    ) -> Self {
        // Decompose every fact into region facts and type facts.
        let mut type_facts = Vec::new();

        let mut edges = Vec::new();
        let mut lifetimes = FxHashMap::default();

        // `'static` gets the `0` index and outlives every other lifetime.
        lifetimes.insert(Ty::new_lifetime(Lifetime::Static, engine), 0);

        let mut index_of = |lifetime: &Interned<Ty>| {
            let next = lifetimes.len();
            *lifetimes.entry(lifetime.clone()).or_insert(next)
        };

        for fact in facts {
            for component in Ty::outlives_components(fact.lesser(), engine).await {
                match component {
                    OutlivesComponent::Region(region) => {
                        edges.push((index_of(&region), index_of(fact.greater())));
                    }
                    OutlivesComponent::Param(subject) | OutlivesComponent::Projection(subject) => {
                        type_facts.push(OutlivesPredicate::new(subject, fact.greater().clone()));
                    }
                }
            }
        }

        let size = lifetimes.len();
        let static_outlives_all = (1..size).map(|index| (0, index));

        let closure =
            TransitiveClosure::new(edges.into_iter().chain(static_outlives_all), size, true)
                .expect("every edge refers to an indexed lifetime");

        Self { lifetimes, closure, type_facts }
    }

    /// Iterates over the lifetimes the region facts mention, in unspecified
    /// order. `'static` is always one of them.
    pub fn lifetimes(&self) -> impl Iterator<Item = &Interned<Ty>> { self.lifetimes.keys() }

    /// Returns whether `less: greater` follows from the region facts.
    ///
    /// Erased lifetimes and errors always satisfy the relation: the former
    /// are checked on the IR, and the latter were already reported.
    #[must_use]
    pub fn region_outlives(&self, less: &Interned<Ty>, greater: &Interned<Ty>) -> bool {
        if less == greater || !less.is_checked_lifetime() || !greater.is_checked_lifetime() {
            return true;
        }

        let Some(&less) = self.lifetimes.get(less) else {
            return false;
        };
        let has_path = |shorter| self.closure.has_path(less, shorter).unwrap_or(false);

        // A lifetime known to outlive `'static` outlives every lifetime, even
        // one that no fact mentions.
        has_path(0) || self.lifetimes.get(greater).is_some_and(|&less| has_path(less))
    }
}

/// The outlives relations between lifetimes that an entailment may rely on.
///
/// The facts of an [`OutlivesEnvironment`] are stated over the lifetimes a
/// declaration names. A caller that knows more, such as the borrow checker,
/// which relates the regions of a function body, tells what holds between
/// two lifetimes through this trait.
pub trait RegionRelation {
    /// Returns whether `'lesser: 'greater` holds.
    fn outlives(&self, lesser: &Interned<Ty>, greater: &Interned<Ty>) -> bool;

    /// Returns whether every one of `constraints` holds.
    fn all_outlive(&self, constraints: &OutlivesConstraints) -> bool {
        constraints
            .iter()
            .all(|constraint| self.outlives(constraint.lesser(), constraint.greater()))
    }
}

impl RegionRelation for OutlivesEnvironment {
    fn outlives(&self, lesser: &Interned<Ty>, greater: &Interned<Ty>) -> bool {
        self.region_outlives(lesser, greater)
    }
}

impl crate::Solver {
    /// Returns whether an outlives predicate follows from the facts visible
    /// at this solver's site.
    pub async fn entails_outlives(&mut self, predicate: &OutlivesPredicate) -> bool {
        let environment = self.outlives_environment().clone();
        self.entails_outlives_with(predicate, &*environment).await
    }

    /// Returns whether an outlives predicate follows from the facts visible
    /// at this solver's site, when the lifetimes are related as `regions`
    /// tells rather than by the region facts alone.
    ///
    /// This is for a predicate that mentions lifetimes the facts do not: the
    /// regions of a function body. A type fact still comes from the site,
    /// and applies to a parameter or a projection of the subject when the
    /// two are equal, and `regions` holds of every outlives constraint that
    /// making them equal requires.
    pub async fn entails_outlives_with(
        &mut self,
        predicate: &OutlivesPredicate,
        regions: &(impl RegionRelation + Sync),
    ) -> bool {
        // The given equalities that normalization matched modulo lifetimes
        // only apply when the lifetimes are equal too.
        let (subject, outlives) = self.normalize_with_outlives(predicate.lesser()).await;
        if !regions.all_outlive(&outlives) {
            return false;
        }

        Box::pin(self.entails_type_outlives(&subject, predicate.greater(), regions)).await
    }

    /// Returns whether every component of `ty` outlives `bound`.
    async fn entails_type_outlives(
        &mut self,
        ty: &Interned<Ty>,
        bound: &Interned<Ty>,
        regions: &(impl RegionRelation + Sync),
    ) -> bool {
        // An erased or erroneous bound is satisfied, as in `region_outlives`.
        if !bound.is_universal_region() {
            return true;
        }

        for component in Ty::outlives_components(ty, self.engine()).await {
            let entailed = match &component {
                OutlivesComponent::Region(region) => regions.outlives(region, bound),
                // TODO: consider also entailing a projection's outlives from
                // everything it projects from with
                // `entails_projection_outlives`; see the concern noted there.
                OutlivesComponent::Param(subject) | OutlivesComponent::Projection(subject) => {
                    self.has_type_fact(subject, bound, regions).await
                }
            };
            if !entailed {
                return false;
            }
        }
        true
    }

    /// Returns whether `subject: bound` follows from a type fact of this
    /// solver's site, directly or through `regions`.
    ///
    /// A fact applies when its subject and `subject` are related invariantly
    /// without binding any variable, which sees through normalization, and
    /// `regions` holds of every outlives constraint the relation requires.
    /// The arguments of a projection are invariant, so a fact about
    /// `i.Assoc['x]` says nothing about `i.Assoc['r]` unless `'r` and `'x`
    /// outlive each other.
    async fn has_type_fact(
        &mut self,
        subject: &Interned<Ty>,
        bound: &Interned<Ty>,
        regions: &(impl RegionRelation + Sync),
    ) -> bool {
        let environment = self.outlives_environment().clone();

        for fact in &environment.type_facts {
            if !regions.outlives(fact.greater(), bound) {
                continue;
            }

            // The same type needs no relation.
            if fact.lesser() == subject {
                return true;
            }

            let Some(equalities) =
                Box::pin(self.relate_without_unify(subject, fact.lesser())).await
            else {
                continue;
            };

            if regions.all_outlive(&equalities) {
                return true;
            }
        }

        false
    }

    /// Returns whether a rigid projection outlives `bound` because everything
    /// it projects from does: the instance it projects from, the arguments of
    /// that instance's trait reference, and the projection's own arguments
    /// (Rust RFC 1214).
    #[allow(dead_code)]
    async fn entails_projection_outlives(
        &mut self,
        projection: &Interned<Ty>,
        bound: &Interned<Ty>,
        regions: &(impl RegionRelation + Sync),
    ) -> bool {
        // TODO: Should we allow this entailment? What's differ in Rust is that
        // the to-be-solved instance can mention other lifetimes that doesn't
        // initially appear in `TraitRef`. For instance, if we have a triat-ref
        // `TraitRef['x, 'y]` the instance symbol could potentially write `inst
        // MyInst['x, 'y, 'z] for TraitRef['x, 'y]` where `'z` is a lifetime that
        // doesn't appear in the trait-ref.
        let Some(view) = projection.as_instance_associated_view() else {
            return false;
        };
        let Ok(trait_ref) = view.instance().instance_trait_ref(self.engine()).await else {
            return false;
        };

        let arguments = std::iter::once(view.instance())
            .chain(trait_ref.args().interned_iter())
            .chain(view.args());
        for argument in arguments {
            if !self.entails_type_outlives(argument, bound, regions).await {
                return false;
            }
        }
        true
    }
}

/// Collects the outlives predicates among `givens`.
pub(crate) fn outlives_givens(
    givens: &[PredicateKind],
) -> impl Iterator<Item = &OutlivesPredicate> {
    givens.iter().filter_map(PredicateKind::as_outlives)
}

#[cfg(test)]
mod test;
