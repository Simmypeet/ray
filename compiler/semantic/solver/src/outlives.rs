//! Outlives requirements between named lifetimes.
//!
//! This module answers questions such as `'a: 'b` or `t: 'a` against a fixed
//! set of known facts, without any region inference. The facts are the
//! outlives predicates declared in where clauses and the bounds implied by the
//! well-formedness of a declaration's signature.

use std::collections::BTreeSet;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::{
    instance_trait_ref::get_instance_trait_ref, marker_implementation::get_marker_implementation,
    parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    parent::scope_walker,
    symbol_kind::{SymbolKind, get_symbol_kind},
};
use rayc_transitive_closure::TransitiveClosure;
use rayc_type::{
    outlives::{OutlivesComponent, get_inferred_outlives},
    poly_var::{build_subst_from_args, get_poly_var_map},
    subst::{Subst, Substitutable},
    trait_ref::TraitRef,
    ty::{Ty, application::View as ApplicationView, lifetime::Lifetime},
    where_clause::{OutlivesPredicate, PredicateKind, get_where_clause},
};

/// Collects the outlives requirements that make a type well-formed.
///
/// - `&'a t` requires `t: 'a`;
/// - a struct `S[args]` requires its declared and inferred outlives predicates,
///   instantiated with `args`;
/// - an effect label `E[args]` requires its declared outlives predicates,
///   instantiated with `args`.
///
/// Every other constructor only requires its arguments to be well-formed.
#[derive(Debug)]
pub struct WfOutlivesCollector<'x> {
    engine: &'x TrackedEngine,

    /// Inferred outlives predicates that are still being computed, used in
    /// place of the query for the structs they contain.
    in_progress: Option<&'x FxHashMap<GlobalSymbolID, BTreeSet<OutlivesPredicate>>>,

    requirements: Vec<OutlivesPredicate>,
}

impl<'x> WfOutlivesCollector<'x> {
    /// Creates a collector that reads inferred outlives predicates from the
    /// query.
    #[must_use]
    pub const fn new(engine: &'x TrackedEngine) -> Self {
        Self { engine, in_progress: None, requirements: Vec::new() }
    }

    /// Creates a collector for the inferred outlives fixed point. Structs in
    /// `in_progress` use its current sets instead of the query.
    #[must_use]
    pub const fn with_in_progress(
        engine: &'x TrackedEngine,
        in_progress: &'x FxHashMap<GlobalSymbolID, BTreeSet<OutlivesPredicate>>,
    ) -> Self {
        Self { engine, in_progress: Some(in_progress), requirements: Vec::new() }
    }

    /// Collects the requirements of `ty` and of every type nested in it.
    pub async fn collect(&mut self, ty: &Interned<Ty>) {
        let mut pending = vec![ty.clone()];

        while let Some(ty) = pending.pop() {
            match &*ty {
                Ty::Application(application) => {
                    self.collect_application(application.view()).await;
                    pending.extend(Ty::interned_arguments(&ty).cloned());
                }
                Ty::EffectRow(row) => {
                    for label in row.labels() {
                        let subst = self
                            .engine
                            .build_subst_from_args(
                                label.effect_symbol_id(),
                                label.arguments().interned_iter(),
                            )
                            .await;
                        self.collect_declared(label.effect_symbol_id(), &subst).await;
                    }
                    pending.extend(row.interned_iter().cloned());
                }
                Ty::Inference(_) | Ty::PolyVar(_) | Ty::SelfInstance(_) | Ty::Lifetime(_) => {}
            }
        }
    }

    /// Returns the collected requirements.
    #[must_use]
    pub fn into_requirements(self) -> Vec<OutlivesPredicate> { self.requirements }

    /// Collects the requirements of one type constructor, excluding those of
    /// its arguments.
    async fn collect_application(&mut self, view: ApplicationView<'_>) {
        match view {
            ApplicationView::Reference(reference) => {
                self.requirements.push(OutlivesPredicate::Type {
                    ty: reference.pointee().clone(),
                    bound: reference.lifetime().clone(),
                });
            }
            ApplicationView::Struct(view) => {
                let subst = view.create_subst(self.engine).await;
                self.collect_declared(view.symbol_id(), &subst).await;

                let inferred = match self.in_progress.and_then(|map| map.get(&view.symbol_id())) {
                    Some(inferred) => inferred.iter().cloned().collect::<Vec<_>>(),
                    None => self.engine.get_inferred_outlives(view.symbol_id()).await.to_vec(),
                };
                self.requirements.extend(
                    inferred
                        .iter()
                        .map(|predicate| predicate.apply_subst_or_clone(&subst, self.engine)),
                );
            }
            ApplicationView::Primitive(_)
            | ApplicationView::Tuple(_)
            | ApplicationView::Pointer(_)
            | ApplicationView::Instance(_)
            | ApplicationView::InstanceAssociated(_)
            | ApplicationView::Closure(_)
            | ApplicationView::DefInstance(_)
            | ApplicationView::NoOpDropInstance(_)
            | ApplicationView::TupleDropInstance(_)
            | ApplicationView::ClosureDropInstance(_)
            | ApplicationView::NominalDropInstance(_)
            | ApplicationView::Error => {}
        }
    }

    /// Collects the outlives predicates declared by `symbol_id`, instantiated
    /// with `subst`.
    async fn collect_declared(&mut self, symbol_id: GlobalSymbolID, subst: &Subst) {
        let where_clause = self.engine.get_where_clause(symbol_id).await;
        self.requirements.extend(where_clause.iter().filter_map(|predicate| {
            predicate
                .kind()
                .as_outlives()
                .map(|predicate| predicate.apply_subst_or_clone(subst, self.engine))
        }));
    }
}

/// Retrieves the outlives facts implied at a site: the bounds implied by the
/// well-formedness of the site's signature and of every enclosing
/// declaration's signature.
///
/// - a `def`'s parameter and return types are assumed well-formed;
/// - an instance's trait-reference arguments, and a marker implementation's
///   implementor, are assumed well-formed;
/// - a struct assumes its inferred outlives predicates.
///
/// Declared where clauses are not included; they are part of the givens.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<[OutlivesPredicate]>)]
#[extend(by_val, name = get_implied_bounds)]
pub struct ImpliedBoundsKey {
    pub site: GlobalSymbolID,
}

#[executor(config = Config)]
pub async fn implied_bounds_executor(
    &ImpliedBoundsKey { site }: &ImpliedBoundsKey,
    engine: &TrackedEngine,
) -> Interned<[OutlivesPredicate]> {
    let mut bounds = Vec::new();
    let mut scopes = engine.scope_walker(site);
    while let Some(symbol_id) = scopes.next().await {
        let symbol_id = site.target_id.make_global(symbol_id);
        bounds.extend(own_implied_bounds(symbol_id, engine).await);
    }
    engine.intern_unsized(bounds)
}

#[distributed_slice(RAY_PROGRAM)]
static IMPLIED_BOUNDS_EXECUTOR: Registration<Config> =
    Registration::new::<ImpliedBoundsKey, ImpliedBoundsExecutor>();

/// Returns the bounds implied by one declaration, excluding its enclosing
/// declarations.
async fn own_implied_bounds(
    symbol_id: GlobalSymbolID,
    engine: &TrackedEngine,
) -> Vec<OutlivesPredicate> {
    let mut collector = WfOutlivesCollector::new(engine);
    match engine.get_symbol_kind(symbol_id).await {
        SymbolKind::Def
        | SymbolKind::InstanceDef
        | SymbolKind::TraitDef
        | SymbolKind::EffectOperation => {
            let parameters = engine.get_parameter_map(symbol_id).await;
            for (_, parameter) in parameters.iter() {
                collector.collect(parameter.ty()).await;
            }
            collector.collect(&engine.get_return_type(symbol_id).await).await;
        }
        SymbolKind::Instance => {
            if let Some(trait_ref) = engine.get_instance_trait_ref(symbol_id).await {
                for argument in trait_ref.args().interned_iter() {
                    collector.collect(argument).await;
                }
            }
        }
        SymbolKind::MarkerImplementation => {
            let implementation = engine.get_marker_implementation(symbol_id).await;
            collector.collect(implementation.implementor()).await;
        }
        SymbolKind::Strut => {
            return engine.get_inferred_outlives(symbol_id).await.to_vec();
        }
        SymbolKind::Effect
        | SymbolKind::ExternDef
        | SymbolKind::InstanceType
        | SymbolKind::Marker
        | SymbolKind::Module
        | SymbolKind::Trait
        | SymbolKind::TraitType => {}
    }
    collector.into_requirements()
}

/// The outlives facts known at a declaration site, and the entailment of
/// outlives predicates from them.
#[derive(Debug, Clone)]
pub struct OutlivesEnvironment {
    /// The lifetimes that appear in region facts, indexed for `closure`.
    /// `'static` is always at index zero.
    lifetimes: Vec<Interned<Ty>>, // REVIEW: should we use `FxHashMap` instead?

    /// `closure.has_path(a, b)` means that `lifetimes[a]: lifetimes[b]` is
    /// known.
    closure: TransitiveClosure,

    /// Known `subject: bound` facts whose subject is a type or effect-row
    /// variable, or a projection.
    type_facts: Vec<(Interned<Ty>, Interned<Ty>)>,
}

impl OutlivesEnvironment {
    /// Builds the environment from outlives facts. Each type fact is
    /// decomposed into its components first, so `&'b int32: 'a` becomes
    /// `'b: 'a`.
    pub async fn new(
        facts: impl IntoIterator<Item = OutlivesPredicate>,
        engine: &TrackedEngine,
    ) -> Self {
        // Decompose every fact into region facts and type facts.
        let mut region_facts = Vec::new();
        let mut type_facts = Vec::new();
        for fact in facts {
            match fact {
                OutlivesPredicate::Region { longer, shorter } => {
                    region_facts.push((longer, shorter));
                }
                OutlivesPredicate::Type { ty, bound } => {
                    for component in Ty::outlives_components(&ty, engine).await {
                        match component {
                            OutlivesComponent::Region(region) => {
                                region_facts.push((region, bound.clone()));
                            }
                            OutlivesComponent::Param(subject)
                            | OutlivesComponent::Projection(subject) => {
                                type_facts.push((subject, bound.clone()));
                            }
                        }
                    }
                }
            }
        }

        // Index the lifetimes, with `'static` first.
        let mut lifetimes = vec![Ty::new_lifetime(Lifetime::Static, engine)];
        let mut indices = FxHashMap::default();
        indices.insert(lifetimes[0].clone(), 0);
        let mut index_of = |lifetime: &Interned<Ty>| {
            *indices.entry(lifetime.clone()).or_insert_with(|| {
                lifetimes.push(lifetime.clone());
                lifetimes.len() - 1
            })
        };
        let edges = region_facts
            .iter()
            .map(|(longer, shorter)| (index_of(longer), index_of(shorter)))
            .collect::<Vec<_>>();

        // `'static` outlives every lifetime.
        let size = lifetimes.len();
        let edges = edges.into_iter().chain((1..size).map(|index| (0, index))).collect::<Vec<_>>();
        let closure = TransitiveClosure::new(edges, size, true)
            .expect("every edge refers to an indexed lifetime");

        Self { lifetimes, closure, type_facts }
    }

    /// Returns whether `longer: shorter` follows from the region facts.
    ///
    /// Erased lifetimes and errors always satisfy the relation: the former
    /// are checked on the IR, and the latter were already reported.
    #[must_use]
    pub fn region_outlives(&self, longer: &Interned<Ty>, shorter: &Interned<Ty>) -> bool {
        if longer == shorter || !longer.is_checked_lifetime() || !shorter.is_checked_lifetime() {
            return true;
        }

        let index_of = |lifetime: &Interned<Ty>| self.lifetimes.iter().position(|x| x == lifetime);
        let Some(longer) = index_of(longer) else {
            return false;
        };

        // A lifetime known to outlive `'static` outlives every lifetime.
        self.closure.has_path(longer, 0).unwrap_or(false)
            || index_of(shorter)
                .is_some_and(|shorter| self.closure.has_path(longer, shorter).unwrap_or(false))
    }

    /// Returns whether `subject: bound` is a known type fact, directly or
    /// through the region facts.
    fn has_type_fact(&self, subject: &Interned<Ty>, bound: &Interned<Ty>) -> bool {
        self.type_facts.iter().any(|(known, known_bound)| {
            known == subject && self.region_outlives(known_bound, bound)
        })
    }
}

impl crate::Solver {
    /// Returns whether an outlives predicate follows from the facts visible
    /// at this solver's site: the outlives givens and the implied bounds.
    pub async fn entails_outlives(&mut self, predicate: &OutlivesPredicate) -> bool {
        match predicate {
            OutlivesPredicate::Region { longer, shorter } => {
                self.outlives_environment().await.region_outlives(longer, shorter)
            }
            OutlivesPredicate::Type { ty, bound } => {
                let ty = self.normalize(ty).await;
                Box::pin(self.entails_type_outlives(&ty, bound)).await
            }
        }
    }

    /// Returns whether every component of `ty` outlives `bound`.
    async fn entails_type_outlives(&mut self, ty: &Interned<Ty>, bound: &Interned<Ty>) -> bool {
        // An erased or erroneous bound is satisfied, as in `region_outlives`.
        if !bound.is_checked_lifetime() {
            return true;
        }

        for component in Ty::outlives_components(ty, self.engine()).await {
            let entailed = match &component {
                OutlivesComponent::Region(region) => {
                    self.outlives_environment().await.region_outlives(region, bound)
                }
                OutlivesComponent::Param(param) => {
                    self.outlives_environment().await.has_type_fact(param, bound)
                }
                OutlivesComponent::Projection(projection) => {
                    self.outlives_environment().await.has_type_fact(projection, bound)
                        || Box::pin(self.entails_projection_outlives(projection, bound)).await
                }
            };
            if !entailed {
                return false;
            }
        }
        true
    }

    /// Returns whether a rigid projection outlives `bound` because everything
    /// it projects from does: the arguments of the dictionary's trait
    /// reference and the projection's own arguments (Rust RFC 1214).
    async fn entails_projection_outlives(
        &mut self,
        projection: &Interned<Ty>,
        bound: &Interned<Ty>,
    ) -> bool {
        let Some(view) = projection.as_instance_associated_view() else {
            return false;
        };
        let Some(trait_ref) = dictionary_trait_ref(view.instance(), self.engine()).await else {
            return false;
        };

        // REVIEW: Do we really need to `.collect()` here? Movoer, `view.args()` doesn't
        // include the instance itself. So, it should also include the instance itself.
        let arguments =
            trait_ref.args().interned_iter().chain(view.args()).cloned().collect::<Vec<_>>();
        for argument in arguments {
            if !self.entails_type_outlives(&argument, bound).await {
                return false;
            }
        }
        true
    }
}

/// Returns the trait reference that a rigid dictionary is known to implement.
async fn dictionary_trait_ref(instance: &Interned<Ty>, engine: &TrackedEngine) -> Option<TraitRef> {
    match &**instance {
        Ty::PolyVar(poly_var) => {
            engine.get_poly_var_map(poly_var.parent_id()).await.trait_ref_of(poly_var.id()).cloned()
        }
        Ty::SelfInstance(self_instance) => Some(self_instance.trait_ref(engine).await),
        // REVIEW: actually in `Ty::Application`, there's a `Constant::Instance` that we can
        // retrieve the dictionary from.
        Ty::Application(_) | Ty::Inference(_) | Ty::EffectRow(_) | Ty::Lifetime(_) => None,
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
