use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_semantic_element::drop_plan::{
    DropPlan, DropPlanError, GeneratedDropPlan, NominalDropPlan,
};
use rayc_source_file::GlobalSourceID;
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    core_item::{CoreItem, Key as CoreItemKey},
};
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{
        EnclosingMapsKey, GlobalPolyVarID, Key as PolyVarKey, PolyVar, PolyVarMap, PolyVarStack,
    },
    trait_ref::{InstanceTraitRefKey, TraitRef},
    ty::{Ty, application::View, args::Args},
    where_clause::{Key as WhereClauseKey, WhereClause},
};

use super::{InstanceResolutionError, Solver};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID };
    RelativeSpan { start: location, end: location, source_id: GlobalSourceID::default() }
}

#[derive(Clone, Copy)]
enum FixturePlan {
    Generated,
    Explicit,
    CannotDerive,
}

async fn nominal_drop_fixture(
    plan_kind: FixturePlan,
) -> (TrackedEngine, GlobalSymbolID, GlobalSymbolID, Interned<Ty>, GlobalPolyVarID) {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let target = TargetID::TEST;
    let site = target.make_global(SymbolID::from_u128(1));
    let wrapper = target.make_global(SymbolID::from_u128(2));
    let drop_trait = target.make_global(SymbolID::from_u128(3));
    let explicit = target.make_global(SymbolID::from_u128(4));
    let no_drop = target.make_global(SymbolID::from_u128(5));
    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    // Model Wrapper[t] and a call site generic over an opaque `s` with a lexical
    // given Drop[s]. A concrete type such as int32 would resolve to the
    // built-in no-op Drop before lexical lookup, so the site must be opaque.
    let mut wrapper_params = PolyVarMap::new();
    let t = wrapper_params.insert(PolyVar::new_type(engine.intern_unsized("t"), span())).unwrap();
    let requirement = engine.intern(Ty::PolyVar(GlobalPolyVarID::new(wrapper, t)));
    let wrapper_params = engine.intern(wrapper_params);
    let mut site_params = PolyVarMap::new();
    let s = site_params.insert(PolyVar::new_type(engine.intern_unsized("s"), span())).unwrap();
    let site_ty = Ty::new_poly_var(GlobalPolyVarID::new(site, s), &tracked);
    let given = site_params
        .insert(PolyVar::new_instance(
            engine.intern_unsized("sDrop"),
            TraitRef::new(drop_trait, Args::new([site_ty.clone()], &tracked)),
            span(),
        ))
        .unwrap();
    let site_params = engine.intern(site_params);
    let mut scope = PolyVarStack::new();
    scope.push(site, site_params.clone());
    let scope = engine.intern(scope);

    let plan = match plan_kind {
        FixturePlan::Generated => {
            DropPlan::Generated(GeneratedDropPlan::new(vec![requirement], Vec::new()))
        }
        FixturePlan::Explicit => DropPlan::Explicit(explicit),
        FixturePlan::CannotDerive => {
            DropPlan::CannotDerive(DropPlanError::NonConvergentRequirements)
        }
    };

    let explicit_data = matches!(plan_kind, FixturePlan::Explicit).then(|| {
        let mut parameters = PolyVarMap::new();
        let a = parameters.insert(PolyVar::new_type(engine.intern_unsized("a"), span())).unwrap();
        let a_ty = engine.intern(Ty::PolyVar(GlobalPolyVarID::new(explicit, a)));
        let _ = parameters.insert(PolyVar::new_instance(
            engine.intern_unsized("aDrop"),
            TraitRef::new(drop_trait, Args::new([a_ty.clone()], &tracked)),
            span(),
        ));
        let head_ty = Ty::new_struct(wrapper, Args::new([a_ty], &tracked), &tracked);
        let head = TraitRef::new(drop_trait, Args::new([head_ty], &tracked));
        let where_clause = tracked.intern(WhereClause::new(tracked.intern_unsized([])));
        (engine.intern(parameters), head, where_clause)
    });
    drop(tracked);
    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    // `NoDrop` is checked before any nominal plan; give it a distinct symbol so
    // `Wrapper` is not mistaken for it.
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (CoreItemKey { role: CoreItem::DropTrait }, drop_trait),
        (CoreItemKey { role: CoreItem::NoDropStruct }, no_drop),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        NominalDropPlan { symbol_id: wrapper },
        engine_mut.intern(plan),
    )]))));
    // One executor per key type: a later registration for `PolyVarKey` would
    // replace this one, so the explicit instance's map joins the same table.
    let mut poly_var_maps = HashMap::from([
        (PolyVarKey { symbol_id: wrapper }, wrapper_params),
        (PolyVarKey { symbol_id: site }, site_params),
    ]);
    if let Some((parameters, _, _)) = &explicit_data {
        poly_var_maps.insert(PolyVarKey { symbol_id: explicit }, parameters.clone());
    }
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(poly_var_maps)));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        EnclosingMapsKey { symbol_id: site },
        scope,
    )]))));
    if let Some((_, head, where_clause)) = explicit_data {
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            InstanceTraitRefKey { symbol_id: explicit },
            Some(head),
        )]))));
        engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
            WhereClauseKey { symbol_id: explicit },
            where_clause,
        )]))));
    }

    (engine.tracked().await, site, wrapper, site_ty, GlobalPolyVarID::new(site, given))
}

// input: Drop[Wrapper[s]] for an opaque site variable `s`
// premise: Wrapper[t] requires Drop[t], and sDrop is the lexical Drop[s]
// output: NominalDropInstance[Wrapper[s], sDrop], with no obligations
#[tokio::test]
async fn generated_nominal_drop_retains_selected_external_dictionary() {
    let (engine, site, wrapper, site_ty, given) =
        nominal_drop_fixture(FixturePlan::Generated).await;
    let nominal = Ty::new_struct(wrapper, Args::new([site_ty], &engine), &engine);
    let drop_trait = TargetID::TEST.make_global(SymbolID::from_u128(3));
    let required = TraitRef::new(drop_trait, Args::new([nominal.clone()], &engine));
    let mut solver = Solver::with_givens(engine.clone(), site, []).await;

    let (term, obligations, _) = solver.resolve_instance(required).await.unwrap().into_parts();
    let Ty::Application(application) = &*term else { panic!("expected an instance application") };
    let View::NominalDropInstance(instance) = application.view() else {
        panic!("expected a generated nominal Drop dictionary")
    };
    assert_eq!(instance.nominal(), &nominal);
    assert_eq!(instance.external_instances(), &[Ty::new_poly_var(given, &engine)]);
    assert!(obligations.is_empty());
}

// input: Drop[Wrapper[s]]
// premise: Wrapper's plan is CannotDerive
// output: NoInstance; no generated dictionary or global fallback is selected
#[tokio::test]
async fn invalid_nominal_drop_plan_cannot_resolve() {
    let (engine, site, wrapper, site_ty, _) = nominal_drop_fixture(FixturePlan::CannotDerive).await;
    let nominal = Ty::new_struct(wrapper, Args::new([site_ty], &engine), &engine);
    let drop_trait = TargetID::TEST.make_global(SymbolID::from_u128(3));
    let required = TraitRef::new(drop_trait, Args::new([nominal], &engine));
    let mut solver = Solver::with_givens(engine, site, []).await;

    assert_eq!(
        solver.resolve_instance(required.clone()).await,
        Err(InstanceResolutionError::NoInstance { required, failed_candidates: Vec::new() })
    );
}

// input: Drop[Wrapper[s]]
// premise: Wrapper's plan selects DropWrapper[a] given Drop[a], with sDrop in
// scope
// output: DropWrapper[s, sDrop], without querying or ranking global candidates
#[tokio::test]
async fn explicit_nominal_drop_uses_only_planned_instance() {
    let (engine, site, wrapper, site_ty, given) = nominal_drop_fixture(FixturePlan::Explicit).await;
    let nominal = Ty::new_struct(wrapper, Args::new([site_ty.clone()], &engine), &engine);
    let drop_trait = TargetID::TEST.make_global(SymbolID::from_u128(3));
    let explicit = TargetID::TEST.make_global(SymbolID::from_u128(4));
    let required = TraitRef::new(drop_trait, Args::new([nominal], &engine));
    let mut solver = Solver::with_givens(engine.clone(), site, []).await;

    let (term, obligations, _) = solver.resolve_instance(required).await.unwrap().into_parts();
    assert_eq!(
        term,
        Ty::new_instance(
            explicit,
            Args::new([site_ty, Ty::new_poly_var(given, &engine)], &engine),
            &engine,
        )
    );
    assert!(obligations.is_empty());
}

// input: Drop[int32] at a site whose scope has a lexical given Drop[int32]
// premise: built-in Drop for primitives is selected before lexical lookup
// output: NoOpDropInstance[int32]; the lexical given is not used
#[tokio::test]
async fn built_in_drop_takes_precedence_over_lexical_given() {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let target = TargetID::TEST;
    let site = target.make_global(SymbolID::from_u128(1));
    let drop_trait = target.make_global(SymbolID::from_u128(3));
    let no_drop = target.make_global(SymbolID::from_u128(5));
    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let int_ty = Ty::new_primitive(
        rayc_type::ty::Primitive::Integer(rayc_type::ty::Integer::Int32),
        &tracked,
    );

    // The site declares `given (intDrop: Drop[int32])`.
    let mut site_params = PolyVarMap::new();
    let _ = site_params.insert(PolyVar::new_instance(
        engine.intern_unsized("intDrop"),
        TraitRef::new(drop_trait, Args::new([int_ty.clone()], &tracked)),
        span(),
    ));
    let site_params = engine.intern(site_params);
    let mut scope = PolyVarStack::new();
    scope.push(site, site_params.clone());
    let scope = engine.intern(scope);
    drop(tracked);

    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (CoreItemKey { role: CoreItem::DropTrait }, drop_trait),
        (CoreItemKey { role: CoreItem::NoDropStruct }, no_drop),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        PolyVarKey { symbol_id: site },
        site_params,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        EnclosingMapsKey { symbol_id: site },
        scope,
    )]))));

    let engine = engine.tracked().await;
    let required = TraitRef::new(drop_trait, Args::new([int_ty.clone()], &engine));
    let mut solver = Solver::with_givens(engine.clone(), site, []).await;

    let (term, obligations, _) = solver.resolve_instance(required).await.unwrap().into_parts();
    assert_eq!(term, Ty::new_no_op_drop_instance(int_ty, &engine));
    assert!(obligations.is_empty());
}
