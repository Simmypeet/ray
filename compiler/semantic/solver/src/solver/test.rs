use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::{
    constraint::ty_relate::TyRelate,
    subst::Subst,
    trait_ref::TraitRef,
    ty::{Primitive, Ty, TyKind, args::Args},
};

use super::{Solver, TyRelatingEnvironment};

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

// input: Trait[(a,), a] matched against Trait[(int32,), int32]
// premise: a is a polymorphic type variable in the head
// output: {a -> int32}
#[tokio::test]
async fn head_match_binds_nested_and_repeated_head_variables() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::new(engine.clone());
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

    assert_eq!(solver.head_match(&head, &expected).await, Some(Subst::new_singleton(a, int_ty)));
}

// input: Trait[a, a] matched against Trait[bool, int32]
// premise: a must have one consistent binding
// output: None
#[tokio::test]
async fn head_match_rejects_inconsistent_head_bindings() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::new(engine.clone());
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

    assert_eq!(solver.head_match(&head, &expected).await, None);
}

// input: Trait[int32] matched against Trait[a]
// premise: polymorphic variables in the expected reference are rigid
// output: None
#[tokio::test]
async fn head_match_does_not_bind_expected_variables() {
    let (engine, a) = engine_with_type_poly_var().await;
    let mut solver = Solver::new(engine.clone());
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let head =
        TraitRef::new(trait_id, Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine));
    let expected = TraitRef::new(trait_id, Args::new([engine.intern(Ty::PolyVar(a))], &engine));

    assert_eq!(solver.head_match(&head, &expected).await, None);
}

// input: Trait[] matched against OtherTrait[] or Trait[int32]
// premise: trait identity and argument count must agree
// output: None for either mismatch
#[tokio::test]
async fn head_match_rejects_trait_identity_and_arity_mismatches() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::new(engine.clone());
    let trait_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let head = TraitRef::new(trait_id, Args::new([], &engine));
    for expected in [
        TraitRef::new(TargetID::TEST.make_global(SymbolID::from_u128(2)), Args::new([], &engine)),
        TraitRef::new(trait_id, Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine)),
    ] {
        assert_eq!(solver.head_match(&head, &expected).await, None);
    }
}

// input: (a,) <: (int32,), b <: a
// premise: normal inference allows a and b to bind
// output: {a -> int32, b -> int32}
#[tokio::test]
async fn exhaustive_solve_composes_bindings_from_derived_constraints() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::new(engine.clone());
    let a = solver.new_inference(TyKind::Star);
    let b = solver.new_inference(TyKind::Star);
    let a_ty = engine.intern(Ty::Inference(a));
    let b_ty = engine.intern(Ty::Inference(b));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let constrs = vec![
        TyRelate::new(
            Ty::new_tuple(engine.intern_unsized([a_ty.clone()]), &engine),
            Ty::new_tuple(engine.intern_unsized([int_ty.clone()]), &engine),
        ),
        TyRelate::new(b_ty, a_ty),
    ];

    assert_eq!(
        solver.exhaustive_solve(constrs, &TyRelatingEnvironment::Normal).await,
        Some([(a, int_ty.clone()), (b, int_ty)].into_iter().collect::<Subst>())
    );
}

// input: a <: bool, a <: int32
// premise: normal inference requires consistent bindings
// output: None
#[tokio::test]
async fn exhaustive_solve_rejects_conflicting_bindings() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::new(engine.clone());
    let a = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let constrs = vec![
        TyRelate::new(a.clone(), Ty::new_primitive(Primitive::Bool, &engine)),
        TyRelate::new(a, Ty::new_primitive(Primitive::Int32, &engine)),
    ];

    assert_eq!(solver.exhaustive_solve(constrs, &TyRelatingEnvironment::Normal).await, None);
}

// input: a <: int32
// premise: top-level matching cannot bind inference variables
// output: None
#[tokio::test]
async fn exhaustive_solve_respects_the_relating_environment() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::new(engine.clone());
    let a = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let constrs = vec![TyRelate::new(a, Ty::new_primitive(Primitive::Int32, &engine))];

    assert_eq!(
        solver.exhaustive_solve(constrs, &TyRelatingEnvironment::TopLevelMatching).await,
        None
    );
}
