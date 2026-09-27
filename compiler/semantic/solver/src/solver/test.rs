use qbice::storage::intern::Interned;
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::{
    constraint::{outlives::OutlivesConstraints, ty_relate::TyRelate},
    subst::Subst,
    trait_ref::TraitRef,
    ty::{Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel},
    variance::Variance,
};

use super::{Solution, Solver, TyRelatingEnvironment};

fn effect_label(id: u128, engine: &rayc_qbice::TrackedEngine) -> Interned<EffectLabel> {
    let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(id));
    engine.intern(EffectLabel::new(symbol_id, Args::new([], engine)))
}

async fn engine_with_type_poly_var()
-> (rayc_qbice::TrackedEngine, rayc_type::poly_var::GlobalPolyVarID) {
    use std::{collections::HashMap, sync::Arc};

    use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
    use rayc_type::poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap};

    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let parent_id = TargetID::TEST.make_global(SymbolID::from_u128(0));
    let location = RelativeLocation {
        offset: 0,
        mode: OffsetMode::Start,
        relative_to: rayc_arena::ID::new(0),
    };
    let mut poly_vars = PolyVarMap::new();
    let id = poly_vars
        .insert(PolyVar::new_type(engine.intern_unsized("a"), RelativeSpan {
            start: location,
            end: location,
            source_id: TargetID::TEST.make_global(rayc_source_file::LocalSourceID::new(0, 0)),
        }))
        .unwrap();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        rayc_type::poly_var::Key { symbol_id: parent_id },
        engine.intern(poly_vars),
    )]))));
    (Arc::new(engine).tracked().await, GlobalPolyVarID::new(parent_id, id))
}

// input: {IO, State}, {State, IO}
// premise: closed effect rows compare independently of label order
// output: true
#[tokio::test]
async fn equality_without_unification_accepts_semantically_equal_types() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let state = effect_label(2, &engine);
    let left = Ty::new_effect_row([io.clone(), state.clone()], None, &engine);
    let right = Ty::new_effect_row([state, io], None, &engine);
    let mut solver = Solver::without_givens(engine).await;

    assert!(solver.eq_without_unify(&left, &right).await);
}

// input: ?a, int32
// premise: the relation succeeds only by binding ?a to int32
// output: false
#[tokio::test]
async fn equality_without_unification_rejects_a_generated_substitution() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let inference = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);

    assert!(!solver.eq_without_unify(&inference, &int_ty).await);
}

// input: Trait[(a,), a] matched against Trait[(int32,), int32]
// premise: a is a polymorphic type variable in the head
// output: {a -> int32}
#[tokio::test]
async fn head_match_binds_nested_and_repeated_head_variables() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let a_ty = engine.intern(Ty::PolyVar(a));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let head = TraitRef::new(
        trait_id,
        Args::new([Ty::new_tuple(engine.intern_unsized([a_ty.clone()]), &engine), a_ty], &engine),
    );
    let expected = TraitRef::new(
        trait_id,
        Args::new(
            [Ty::new_tuple(engine.intern_unsized([int_ty.clone()]), &engine), int_ty.clone()],
            &engine,
        ),
    );

    assert_eq!(
        solver.type_head_match(&head, &expected).await.map(Solution::into_parts),
        Some((Subst::new_singleton(a, int_ty), OutlivesConstraints::new()))
    );
}

// input: Trait[a, a] matched against Trait[bool, int32]
// premise: a must have one consistent binding
// output: None
#[tokio::test]
async fn head_match_rejects_inconsistent_head_bindings() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let a_ty = engine.intern(Ty::PolyVar(a));
    let head = TraitRef::new(trait_id, Args::new([a_ty.clone(), a_ty], &engine));
    let expected = TraitRef::new(
        trait_id,
        Args::new(
            [
                Ty::new_primitive(Primitive::Bool, &engine),
                Ty::new_primitive(Primitive::Int32, &engine),
            ],
            &engine,
        ),
    );

    assert_eq!(solver.type_head_match(&head, &expected).await, None);
}

// input: Trait[int32] matched against Trait[a]
// premise: polymorphic variables in the expected reference are rigid
// output: None
#[tokio::test]
async fn head_match_does_not_bind_expected_variables() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let head =
        TraitRef::new(trait_id, Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine));
    let expected = TraitRef::new(trait_id, Args::new([engine.intern(Ty::PolyVar(a))], &engine));

    assert_eq!(solver.type_head_match(&head, &expected).await, None);
}

// input: Trait[] matched against OtherTrait[] or Trait[int32]
// premise: trait identity and argument count must agree
// output: None for either mismatch
#[tokio::test]
async fn head_match_rejects_trait_identity_and_arity_mismatches() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let head = TraitRef::new(trait_id, Args::new([], &engine));
    for expected in [
        TraitRef::new(TargetID::TEST.make_global(SymbolID::from_u128(2)), Args::new([], &engine)),
        TraitRef::new(trait_id, Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine)),
    ] {
        assert_eq!(solver.type_head_match(&head, &expected).await, None);
    }
}

// input: (a,) <: (int32,), b <: a
// premise: normal inference allows a and b to bind
// output: {a -> int32, b -> int32}
#[tokio::test]
async fn exhaustive_solve_composes_bindings_from_derived_constraints() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let a = solver.new_inference(TyKind::Star);
    let b = solver.new_inference(TyKind::Star);
    let a_ty = engine.intern(Ty::Inference(a));
    let b_ty = engine.intern(Ty::Inference(b));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let constrs = vec![
        TyRelate::new_invariant(
            Ty::new_tuple(engine.intern_unsized([a_ty.clone()]), &engine),
            Ty::new_tuple(engine.intern_unsized([int_ty.clone()]), &engine),
        ),
        TyRelate::new_invariant(b_ty, a_ty),
    ];

    assert_eq!(
        solver
            .exhaustive_solve(constrs, &TyRelatingEnvironment::Normal)
            .await
            .map(Solution::into_parts),
        Some((
            [(a, int_ty.clone()), (b, int_ty)].into_iter().collect::<Subst>(),
            OutlivesConstraints::new()
        ))
    );
}

// input: a <: bool, a <: int32
// premise: normal inference requires consistent bindings
// output: None
#[tokio::test]
async fn exhaustive_solve_rejects_conflicting_bindings() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let a = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let constrs = vec![
        TyRelate::new_invariant(a.clone(), Ty::new_primitive(Primitive::Bool, &engine)),
        TyRelate::new_invariant(a, Ty::new_primitive(Primitive::Int32, &engine)),
    ];

    assert_eq!(solver.exhaustive_solve(constrs, &TyRelatingEnvironment::Normal).await, None);
}

// input: a <: int32
// premise: top-level matching cannot bind inference variables
// output: None
#[tokio::test]
async fn exhaustive_solve_respects_the_relating_environment() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let a = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let constrs = vec![TyRelate::new_invariant(a, Ty::new_primitive(Primitive::Int32, &engine))];

    assert_eq!(
        solver.exhaustive_solve(constrs, &TyRelatingEnvironment::TopLevelMatching).await,
        None
    );
}

// input: self of Trait[a] checked against Trait[a] and Trait[int32]
// premise: the self dictionary carries the trait's identity parameters
// output: the identity reference is accepted; specialization without
// substitution fails
#[tokio::test]
async fn self_instance_entails_its_identity_trait_reference() {
    use rayc_type::{
        constraint::instance_trait_ref::InstanceTraitRef, ty::self_instance::SelfInstance,
    };

    use crate::ty_relate::{DerivedConstraint, Error, Step};
    let (engine, a) = engine_with_type_poly_var().await;
    let this = SelfInstance::new(a.parent_id());
    let identity =
        TraitRef::new(a.parent_id(), Args::new([engine.intern(Ty::PolyVar(a))], &engine));
    assert_eq!(this.trait_ref(&engine).await, identity);
    let instance = engine.intern(Ty::SelfInstance(this));
    let mut solver = Solver::without_givens(engine.clone()).await;
    let result =
        solver.entail_instance_trait_ref(&InstanceTraitRef::new(instance.clone(), identity)).await;
    let a_ty = engine.intern(Ty::PolyVar(a));
    assert_eq!(
        result,
        Ok(Step::Derived(vec![DerivedConstraint::new_type_application_matching(
            a_ty.clone(),
            a_ty,
            Variance::Invariant,
        ),]))
    );
    let specialized = TraitRef::new(
        a.parent_id(),
        Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine),
    );
    let result = solver
        .entail_instance_trait_ref(&InstanceTraitRef::new(instance, specialized))
        .await
        .unwrap();
    let Step::Derived(constraints) = result else { panic!("expected argument constraints") };
    for constraint in constraints {
        assert_eq!(
            solver.entail_ty_relate(&constraint.ty_relate).await.map(|_| ()),
            Err(Error::Conflicted)
        );
    }
}

// input: a solver at a trait method, nested inside a trait and module
// premise: method gives Item = int32; trait gives Item = bool and Other = bool
// output: the local Item equality wins; the inherited Other equality is visible
#[tokio::test]
async fn site_givens_include_parents_and_prefer_the_nearest_scope() {
    use std::{collections::HashMap, sync::Arc};

    use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
    use rayc_symbol::{
        parent,
        symbol_kind::{self, SymbolKind},
    };
    use rayc_type::{
        ty::self_instance::SelfInstance,
        where_clause::{AssociatedTypeEquality, Key, Predicate, PredicateKind, WhereClause},
    };

    let (types, _) = engine_with_type_poly_var().await;
    let module = TargetID::TEST.make_global(SymbolID::from_u128(10));
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(11));
    let site = TargetID::TEST.make_global(SymbolID::from_u128(12));
    let dictionary = types.intern(Ty::SelfInstance(SelfInstance::new(owner)));
    let item = Ty::new_instance_associated(site, dictionary.clone(), [], &types);
    let other = Ty::new_instance_associated(module, dictionary, [], &types);
    let int_ty = Ty::new_primitive(Primitive::Int32, &types);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &types);
    let location = RelativeLocation {
        offset: 0,
        mode: OffsetMode::Start,
        relative_to: rayc_arena::ID::new(0),
    };
    let span = RelativeSpan {
        start: location,
        end: location,
        source_id: TargetID::TEST.make_global(rayc_source_file::LocalSourceID::new(0, 0)),
    };
    let predicate = |left, right| {
        Predicate::new(
            PredicateKind::AssociatedTypeEquality(AssociatedTypeEquality::new(left, right)),
            span,
            rayc_type::where_clause::PredicateOrigin::Declared,
        )
    };
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (parent::Key { symbol_id: site }, Some(owner.id)),
        (parent::Key { symbol_id: owner }, Some(module.id)),
        (parent::Key { symbol_id: module }, None),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (symbol_kind::Key { symbol_id: site }, SymbolKind::TraitDef),
        (symbol_kind::Key { symbol_id: owner }, SymbolKind::Trait),
        (symbol_kind::Key { symbol_id: module }, SymbolKind::Module),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (
            Key { symbol_id: site },
            types.intern(WhereClause::new(
                types.intern_unsized([predicate(item.clone(), int_ty.clone())]),
            )),
        ),
        (
            Key { symbol_id: owner },
            types.intern(WhereClause::new(types.intern_unsized([
                predicate(item.clone(), bool_ty.clone()),
                predicate(other.clone(), bool_ty.clone()),
            ]))),
        ),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (rayc_type::associated_type_kind::Key { symbol_id: site }, TyKind::Star),
        (rayc_type::associated_type_kind::Key { symbol_id: module }, TyKind::Star),
    ]))));
    engine.register_executor(Arc::new(crate::givens::GivensExecutor));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        crate::outlives::OutlivesEnvironmentKey { site },
        types.intern(crate::outlives::OutlivesEnvironment::new([], &types).await),
    )]))));
    let solver = Solver::new(Arc::new(engine).tracked().await, site).await;

    assert_eq!(solver.normalize(&item).await, int_ty);
    assert_eq!(solver.normalize(&other).await, bool_ty);
}

// input: Show['static] matched against Show['?0]
// premise: the solver keeps outlives constraints
// output: the head matches with no substitution and requires '?0: 'static
#[tokio::test]
async fn head_match_ignores_lifetimes_but_requires_their_outlives() {
    use rayc_type::{constraint::outlives::OutlivesConstraint, ty::lifetime::Lifetime};

    let engine = rayc_qbice::create_minimal_engine().await;
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let static_ = Ty::new_lifetime(Lifetime::Static, &engine);
    let region = Ty::new_lifetime(Lifetime::Region(rayc_arena::ID::new(0)), &engine);
    let head = TraitRef::new(trait_id, Args::new([static_.clone()], &engine));
    let expected = TraitRef::new(trait_id, Args::new([region.clone()], &engine));
    let mut solver = Solver::without_givens(engine.clone()).await;

    assert_eq!(
        solver.type_head_match(&head, &expected).await.map(Solution::into_parts),
        Some((
            Subst::new_empty(),
            std::iter::once(OutlivesConstraint::new(region, static_)).collect()
        ))
    );
}
