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

    /// Returns whether `subject: bound` is a known type fact, directly or
    /// through the region facts.
    fn has_type_fact(&self, subject: &Interned<Ty>, bound: &Interned<Ty>) -> bool {
        // TODO: once we have subtyping TyRelate, we'll migrate from `==` syntactic
        // equality to a proper subtyping relationship.
        self.type_facts
            .iter()
            .any(|fact| fact.lesser() == subject && self.region_outlives(fact.greater(), bound))
    }
}

impl crate::Solver {
    /// Returns whether an outlives predicate follows from the facts visible
    /// at this solver's site.
    pub async fn entails_outlives(&mut self, predicate: &OutlivesPredicate) -> bool {
        let subject = self.normalize(predicate.lesser()).await;
        Box::pin(self.entails_type_outlives(&subject, predicate.greater())).await
    }

    /// Returns whether every component of `ty` outlives `bound`.
    async fn entails_type_outlives(&mut self, ty: &Interned<Ty>, bound: &Interned<Ty>) -> bool {
        // An erased or erroneous bound is satisfied, as in `region_outlives`.
        if !bound.is_checked_lifetime() {
            return true;
        }

        for component in Ty::outlives_components(ty, self.engine()).await {
            let environment = self.outlives_environment();
            let entailed = match &component {
                OutlivesComponent::Region(region) => environment.region_outlives(region, bound),
                OutlivesComponent::Param(param) => environment.has_type_fact(param, bound),
                // TODO: consider also entailing a projection's outlives from
                // everything it projects from with
                // `entails_projection_outlives`; see the concern noted there.
                OutlivesComponent::Projection(projection) => {
                    environment.has_type_fact(projection, bound)
                }
            };
            if !entailed {
                return false;
            }
        }
        true
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
            if !self.entails_type_outlives(argument, bound).await {
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
