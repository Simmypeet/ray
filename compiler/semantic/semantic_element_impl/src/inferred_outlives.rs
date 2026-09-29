//! Outlives requirements inferred for structs from their field types, as in
//! Rust RFC 2093.
//!
//! `struct Ref['a, t]: value: &'a t` infers `t: 'a`, and a struct holding a
//! `Ref['a, t]` infers the same requirement in turn. References are the only
//! source: a declared outlives predicate is never inferred. Structs can be
//! recursive and mutually recursive, so the requirements of every struct in a
//! target are computed together as one fixed point.

use std::collections::BTreeSet;

use linkme::distributed_slice;
use qbice::{
    Decode, Encode, Query, StableHash, executor, program::Registration, storage::intern::Interned,
};
use rayc_hash::FxHashMap;
use rayc_qbice::{Config, RAY_PROGRAM, TrackedEngine};
use rayc_semantic_element::struct_body::get_struct_body;
use rayc_solver::outlives::implied::ImpliedOutlivesCollector;
use rayc_symbol::{GlobalSymbolID, SymbolID, symbol_kind::get_all_nominal_type_ids};
use rayc_target::TargetID;
use rayc_type::{outlives::InferredOutlivesKey, ty::Ty, where_clause::OutlivesPredicate};

/// Retrieves the inferred outlives requirements of every struct in a target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, Query,
)]
#[value(Interned<FxHashMap<SymbolID, Interned<[OutlivesPredicate]>>>)]
struct InferredOutlivesMapKey {
    target_id: TargetID,
}

#[executor(config = Config)]
async fn inferred_outlives_map_executor(
    &InferredOutlivesMapKey { target_id }: &InferredOutlivesMapKey,
    engine: &TrackedEngine,
) -> Interned<FxHashMap<SymbolID, Interned<[OutlivesPredicate]>>> {
    // Start every struct in the target with no requirements.
    let structs = engine
        .get_all_nominal_type_ids(target_id)
        .await
        .iter()
        .map(|id| target_id.make_global(*id))
        .collect::<Vec<_>>();
    let mut inferred = structs
        .iter()
        .map(|symbol_id| (*symbol_id, BTreeSet::new()))
        .collect::<FxHashMap<_, BTreeSet<OutlivesPredicate>>>();

    // Grow the sets until no struct gains a requirement. The sets only grow,
    // and each is bounded by the struct's own lifetimes and variables, so
    // this terminates.
    let mut changed = true;
    while changed {
        changed = false;
        for struct_id in &structs {
            changed |= grow_requirements(*struct_id, &mut inferred, engine).await;
        }
    }

    engine.intern(
        inferred
            .into_iter()
            .map(|(struct_id, predicates)| {
                (struct_id.id, engine.intern_unsized(predicates.into_iter().collect::<Vec<_>>()))
            })
            .collect(),
    )
}

#[distributed_slice(RAY_PROGRAM)]
static INFERRED_OUTLIVES_MAP_EXECUTOR: Registration<Config> =
    Registration::new::<InferredOutlivesMapKey, InferredOutlivesMapExecutor>();

#[executor(config = Config)]
async fn inferred_outlives_executor(
    &InferredOutlivesKey { struct_id }: &InferredOutlivesKey,
    engine: &TrackedEngine,
) -> Interned<[OutlivesPredicate]> {
    let map = engine.query(&InferredOutlivesMapKey { target_id: struct_id.target_id }).await;
    map.get(&struct_id.id).cloned().expect("incorrect key")
}

#[distributed_slice(RAY_PROGRAM)]
static INFERRED_OUTLIVES_EXECUTOR: Registration<Config> =
    Registration::new::<InferredOutlivesKey, InferredOutlivesExecutor>();

/// Adds to the set of `struct_id` in `inferred` the requirements that its
/// field types place on its own lifetimes and variables, given the current
/// sets. Returns whether the set gained a requirement.
async fn grow_requirements(
    struct_id: GlobalSymbolID,
    inferred: &mut FxHashMap<GlobalSymbolID, BTreeSet<OutlivesPredicate>>,
    engine: &TrackedEngine,
) -> bool {
    // Collect the bounds implied by every field type.
    let body = engine.get_struct_body(struct_id).await;
    let mut collector = ImpliedOutlivesCollector::with_in_progress(engine, inferred);
    for (_, field) in body.iter() {
        collector.collect(field.ty()).await;
    }
    let requirements = collector.into_requirements();

    // Keep the components that only mention the struct's own variables and
    // that do not hold trivially.
    let set = inferred.get_mut(&struct_id).expect("every struct has a set");
    let mut changed = false;
    for requirement in requirements {
        for predicate in decompose(requirement, engine).await {
            if is_inferable(&predicate, struct_id) {
                changed |= set.insert(predicate);
            }
        }
    }
    changed
}

/// Decomposes a requirement into one requirement per outlives component, so
/// `&'b int32: 'a` becomes `'b: 'a`.
async fn decompose(
    requirement: OutlivesPredicate,
    engine: &TrackedEngine,
) -> impl Iterator<Item = OutlivesPredicate> {
    Ty::outlives_components(requirement.subject(), engine).await.into_iter().map(move |component| {
        OutlivesPredicate::new(component.ty().clone(), requirement.bound().clone())
    })
}

/// Returns whether a decomposed requirement can be stated over the struct's
/// own variables and does not hold trivially.
fn is_inferable(predicate: &OutlivesPredicate, struct_id: GlobalSymbolID) -> bool {
    let subject = predicate.subject();
    let bound = predicate.bound();

    // `'a: 'a` holds trivially, and erased or erroneous lifetimes are never
    // checked here.
    if subject == bound || !subject.is_checked_lifetime() || !bound.is_checked_lifetime() {
        return false;
    }

    // Every variable must be one of the struct's own.
    [subject, bound].into_iter().all(|ty| {
        ty.recursive_iter().all(|ty| ty.as_poly_var().is_none_or(|id| id.parent_id() == struct_id))
    })
}
