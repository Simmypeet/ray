use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    subst::{Subst, Substitutable},
    ty::{Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel, inference::Inference},
};

use super::{Solver, TyRelate};
use crate::ty_relate::{DerivedConstraint, Error, Step};

fn effect_label(id: u128, engine: &TrackedEngine) -> Interned<EffectLabel> {
    effect_label_with_args(id, [], engine)
}

fn effect_label_with_args(
    id: u128,
    args: impl IntoIterator<Item = Interned<Ty>>,
    engine: &TrackedEngine,
) -> Interned<EffectLabel> {
    let symbol_id = TargetID::TEST.make_global(SymbolID::from_u128(id));
    engine.intern(EffectLabel::new(symbol_id, Args::new(args, engine)))
}

async fn engine_with_effect_poly_var() -> (TrackedEngine, GlobalPolyVarID) {
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
        .insert(PolyVar::new_effect(engine.intern_unsized("p"), RelativeSpan {
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

// input: ?dict = Instance[int32]
// premise: ?dict has kind Instance; equality may put it on either side
// output: exactly ?dict := Instance[int32]
#[tokio::test]
async fn instance_inference_binds_to_concrete_instance() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let symbol = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let instance = Ty::new_instance(
        symbol,
        Args::new([Ty::new_primitive(Primitive::Int32, &engine)], &engine),
        &engine,
    );
    for reverse in [false, true] {
        let mut solver = Solver::without_givens(engine.clone()).await;
        let inference = solver.new_inference(TyKind::Instance);
        let variable = engine.intern(Ty::Inference(inference));
        let (left, right) =
            if reverse { (instance.clone(), variable) } else { (variable, instance.clone()) };
        assert_eq!(
            solver.entail_ty_relate(&TyRelate::new(left, right)).await,
            Ok(Step::Subst(Subst::new_singleton(inference, instance.clone())))
        );
    }
}

// input: ?k = Error(k)
// premise: ?k is unconstrained and k is Star, Instance, or EffectRow
// output: exactly ?k := Error(k)
#[tokio::test]
async fn inference_binds_to_error_of_its_kind() {
    let engine = rayc_qbice::create_minimal_engine().await;
    for kind in [TyKind::Star, TyKind::Instance, TyKind::EffectRow] {
        let mut solver = Solver::without_givens(engine.clone()).await;
        let inference = solver.new_inference(kind);
        let variable = engine.intern(Ty::Inference(inference));
        let error = Ty::new_error(kind, &engine);
        assert_eq!(
            solver.entail_ty_relate(&TyRelate::new(variable, error.clone())).await,
            Ok(Step::Subst(Subst::new_singleton(inference, error)))
        );
    }
}

// input: ?k = int32, Instance[], {}, or Error(j)
// premise: k differs from the concrete type's kind, in either equality
// direction output: Conflicted
#[tokio::test]
async fn inference_rejects_cross_kind_bindings() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let symbol = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let kinds = [TyKind::Star, TyKind::Instance, TyKind::EffectRow];
    let concrete = [
        (TyKind::Star, Ty::new_primitive(Primitive::Int32, &engine)),
        (TyKind::Instance, Ty::new_instance(symbol, Args::new([], &engine), &engine)),
        (TyKind::EffectRow, Ty::new_effect_row([], None, &engine)),
    ];
    for (kind, ty) in
        concrete.into_iter().chain(kinds.map(|kind| (kind, Ty::new_error(kind, &engine))))
    {
        for inference_kind in kinds.into_iter().filter(|other| *other != kind) {
            for reverse in [false, true] {
                let mut solver = Solver::without_givens(engine.clone()).await;
                let inference = solver.new_inference(inference_kind);
                let variable = engine.intern(Ty::Inference(inference));
                let (left, right) =
                    if reverse { (ty.clone(), variable) } else { (variable, ty.clone()) };
                assert_eq!(
                    solver.entail_ty_relate(&TyRelate::new(left, right)).await,
                    Err(Error::Conflicted)
                );
            }
        }
    }
}

// input: ?numeric or ?equality = a primitive, tuple, or star error
// premise: numeric accepts numbers; equality also accepts bool
// output: a singleton substitution for allowed primitives, otherwise Conflicted
#[tokio::test]
async fn star_application_constraints_remain_enforced() {
    use rayc_type::ty::InferenceConstraint;

    let engine = rayc_qbice::create_minimal_engine().await;
    for (constraint, primitive, allowed) in [
        (InferenceConstraint::Numeric, Primitive::Int32, true),
        (InferenceConstraint::Numeric, Primitive::Float32, true),
        (InferenceConstraint::Numeric, Primitive::CInt, true),
        (InferenceConstraint::Numeric, Primitive::Bool, false),
        (InferenceConstraint::Numeric, Primitive::CStr, false),
        (InferenceConstraint::EqualityComparable, Primitive::Bool, true),
        (InferenceConstraint::EqualityComparable, Primitive::CStr, false),
    ] {
        let mut solver = Solver::without_givens(engine.clone()).await;
        let inference = solver.new_inference_with_constraint(TyKind::Star, constraint);
        let variable = engine.intern(Ty::Inference(inference));
        let ty = Ty::new_primitive(primitive, &engine);
        let expected = if allowed {
            Ok(Step::Subst(Subst::new_singleton(inference, ty.clone())))
        } else {
            Err(Error::Conflicted)
        };
        assert_eq!(solver.entail_ty_relate(&TyRelate::new(variable.clone(), ty)).await, expected);
        for rejected in [Ty::new_unit(&engine), Ty::new_star_error(&engine)] {
            assert_eq!(
                solver.entail_ty_relate(&TyRelate::new(variable.clone(), rejected)).await,
                Err(Error::Conflicted)
            );
        }
    }
}

async fn solve(
    solver: &mut Solver,
    constraint: TyRelate,
    engine: &TrackedEngine,
) -> Result<Subst, Error> {
    let mut pending = vec![constraint];
    let mut subst = Subst::new_empty();

    while let Some(constraint) = pending.pop() {
        let constraint = constraint.apply_subst_or_clone(&subst, engine);
        match solver.entail_ty_relate(&constraint).await? {
            Step::Subst(new_subst) => subst.compose(&new_subst, engine),
            Step::Derived(constraints) => {
                pending.extend(constraints.into_iter().map(|x| x.ty_relate));
            }
            Step::NoProgress => panic!("effect-row constraint should make progress"),
        }
    }

    Ok(subst)
}

// input: {IO, Exn} <: {Exn, IO}
// premise: different effect constructors commute
// output: {}
#[tokio::test]
async fn closed_effect_rows_match_independent_of_label_order() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let exn = effect_label(2, &engine);
    let lesser = Ty::new_effect_row([io.clone(), exn.clone()], None, &engine);
    let greater = Ty::new_effect_row([exn, io], None, &engine);
    let mut solver = Solver::without_givens(engine).await;

    let step = solver.entail_ty_relate(&TyRelate::new(lesser, greater)).await;

    assert_eq!(step, Ok(Step::Derived(Vec::new())));
}

// input: {Exn, Exn} <: {Exn}
// premise: duplicate effect labels are significant
// output: Conflicted
#[tokio::test]
async fn closed_effect_rows_preserve_duplicate_labels() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let exn = effect_label(1, &engine);
    let lesser = Ty::new_effect_row([exn.clone(), exn.clone()], None, &engine);
    let greater = Ty::new_effect_row([exn], None, &engine);
    let mut solver = Solver::without_givens(engine).await;

    let step = solver.entail_ty_relate(&(TyRelate::new(lesser, greater))).await;

    assert_eq!(step, Err(Error::Conflicted));
}

// input: {IO | e1} <: {State | e2}
// premise: e1 and e2 are distinct open effect-row variables
// output: e1 <: {State | e3}, {IO | e3} <: e2
#[tokio::test]
async fn open_effect_rows_share_a_fresh_common_tail() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let state = effect_label(2, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e1 = engine.intern(Ty::Inference(solver.new_inference(TyKind::EffectRow)));
    let e2 = engine.intern(Ty::Inference(solver.new_inference(TyKind::EffectRow)));
    let lesser = Ty::new_effect_row([io.clone()], Some(e1.clone()), &engine);
    let greater = Ty::new_effect_row([state.clone()], Some(e2.clone()), &engine);

    let step = solver.entail_ty_relate(&(TyRelate::new(lesser, greater))).await;

    let e3 = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 2)));
    let state_remainder = Ty::new_effect_row([state], Some(e3.clone()), &engine);
    let io_remainder = Ty::new_effect_row([io], Some(e3), &engine);
    assert_eq!(
        step,
        Ok(Step::Derived(vec![
            DerivedConstraint::new_type_application_matching(e1, state_remainder),
            DerivedConstraint::new_type_application_matching(io_remainder, e2),
        ]))
    );
}

// input: {IO | e1} = {Exn | e2}
// premise: e1 and e2 are distinct open effect-row variables
// output: e1 := {Exn | e3}, e2 := {IO | e3}
#[tokio::test]
async fn distinct_open_effect_rows_have_a_principal_shared_tail_substitution() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let exn = effect_label(2, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e1 = solver.new_inference(TyKind::EffectRow);
    let e2 = solver.new_inference(TyKind::EffectRow);
    let e1_ty = engine.intern(Ty::Inference(e1));
    let e2_ty = engine.intern(Ty::Inference(e2));
    let lesser = Ty::new_effect_row([io.clone()], Some(e1_ty), &engine);
    let greater = Ty::new_effect_row([exn.clone()], Some(e2_ty), &engine);

    let subst = solve(&mut solver, TyRelate::new(lesser, greater), &engine)
        .await
        .expect("distinct open rows should unify through a common tail");

    let e3 = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 2)));
    assert_eq!(subst.get(&e1), Some(&Ty::new_effect_row([exn], Some(e3.clone()), &engine)));
    assert_eq!(subst.get(&e2), Some(&Ty::new_effect_row([io], Some(e3), &engine)));
}

// input: {IO | e} = {IO}
// premise: e is an open effect-row variable
// output: e := {}
#[tokio::test]
async fn open_effect_row_tail_closes_when_no_labels_remain() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e = solver.new_inference(TyKind::EffectRow);
    let e_ty = engine.intern(Ty::Inference(e));
    let open = Ty::new_effect_row([io.clone()], Some(e_ty), &engine);
    let closed = Ty::new_effect_row([io], None, &engine);

    let subst = solve(&mut solver, TyRelate::new(open, closed), &engine)
        .await
        .expect("the open tail should close");

    assert_eq!(subst.get(&e), Some(&Ty::new_effect_row([], None, &engine)));
}

// input: {IO | e} = {IO, IO}
// premise: duplicate effect labels are significant
// output: e := {IO}
#[tokio::test]
async fn duplicate_effect_label_remains_in_open_tail_solution() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e = solver.new_inference(TyKind::EffectRow);
    let e_ty = engine.intern(Ty::Inference(e));
    let open = Ty::new_effect_row([io.clone()], Some(e_ty), &engine);
    let duplicate = Ty::new_effect_row([io.clone(), io.clone()], None, &engine);

    let subst = solve(&mut solver, TyRelate::new(open, duplicate), &engine)
        .await
        .expect("the duplicate label should remain in the tail");

    assert_eq!(subst.get(&e), Some(&Ty::new_effect_row([io], None, &engine)));
}

// input: e = {IO | e}
// premise: e is an effect-row inference variable
// output: OccursCheckFailed
#[tokio::test]
async fn effect_row_inference_cannot_bind_to_a_row_containing_itself() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e = solver.new_inference(TyKind::EffectRow);
    let e_ty = engine.intern(Ty::Inference(e));
    let recursive_row = Ty::new_effect_row([io], Some(e_ty.clone()), &engine);

    let result = solve(&mut solver, TyRelate::new(e_ty, recursive_row), &engine).await;

    assert_eq!(result, Err(Error::OccursCheckFailed));
}

// input: e = p
// premise: e is unconstrained; p is a rigid effect-row variable
// output: e := p, with no substitution for p
#[tokio::test]
async fn effect_inference_binds_to_rigid_effect_poly_var_without_rebinding_it() {
    let (engine, poly) = engine_with_effect_poly_var().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let e = solver.new_inference(TyKind::EffectRow);
    let e_ty = engine.intern(Ty::Inference(e));
    let poly_ty = Ty::new_poly_var(poly, &engine);

    let subst = solve(&mut solver, TyRelate::new(poly_ty.clone(), e_ty), &engine)
        .await
        .expect("an unconstrained inference should bind to a rigid variable");

    assert_eq!(subst.get(&e), Some(&poly_ty));
    assert_eq!(subst.get(&poly), None);
}

// input: {IO | p} = {IO | e}
// premise: p is rigid; e is an effect-row inference variable
// output: e := p
#[tokio::test]
async fn matching_open_effect_rows_unify_their_tails_directly() {
    let (engine, poly) = engine_with_effect_poly_var().await;
    let io = effect_label(1, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let inference = solver.new_inference(TyKind::EffectRow);
    let inference_ty = engine.intern(Ty::Inference(inference));
    let poly_ty = Ty::new_poly_var(poly, &engine);
    let rigid_row = Ty::new_effect_row([io.clone()], Some(poly_ty.clone()), &engine);
    let inferred_row = Ty::new_effect_row([io], Some(inference_ty), &engine);

    let subst = solve(&mut solver, TyRelate::new(rigid_row, inferred_row), &engine)
        .await
        .expect("matching open rows should unify their tails");

    assert_eq!(subst.get(&inference), Some(&poly_ty));
}

// input: e <: {IO}
// premise: e is an effect-row inference variable
// output: e := {IO}
#[tokio::test]
async fn effect_row_inference_binds_to_an_effect_row() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let io = effect_label(1, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let inference = solver.new_inference(TyKind::EffectRow);
    let inference_ty = engine.intern(Ty::Inference(inference));
    let row = Ty::new_effect_row([io], None, &engine);

    let step = solver.entail_ty_relate(&(TyRelate::new(inference_ty, row.clone()))).await;

    assert_eq!(step, Ok(Step::Subst(Subst::new_singleton(inference, row))));
}

// input: {State[int32], State[bool]} = {State[bool], State[int32]}
// premise: occurrences of the same effect constructor cannot commute
// output: Conflicted
#[tokio::test]
async fn same_effect_constructor_occurrences_cannot_swap() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let bool = Ty::new_primitive(Primitive::Bool, &engine);
    let state_int32 = effect_label_with_args(1, [int32], &engine);
    let state_bool = effect_label_with_args(1, [bool], &engine);
    let lesser = Ty::new_effect_row([state_int32.clone(), state_bool.clone()], None, &engine);
    let greater = Ty::new_effect_row([state_bool, state_int32], None, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;

    let result = solve(&mut solver, TyRelate::new(lesser, greater), &engine).await;

    assert_eq!(result, Err(Error::Conflicted));
}

// input: {State[?a], State[?b]} = {State[int32], State[bool]}
// premise: same-constructor occurrences match in row order
// output: ?a := int32, ?b := bool
#[tokio::test]
async fn same_effect_constructor_inferences_bind_in_occurrence_order() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let bool = Ty::new_primitive(Primitive::Bool, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;
    let a = solver.new_inference(TyKind::Star);
    let b = solver.new_inference(TyKind::Star);
    let a_ty = engine.intern(Ty::Inference(a));
    let b_ty = engine.intern(Ty::Inference(b));
    let state_a = effect_label_with_args(1, [a_ty], &engine);
    let state_b = effect_label_with_args(1, [b_ty], &engine);
    let state_int32 = effect_label_with_args(1, [int32.clone()], &engine);
    let state_bool = effect_label_with_args(1, [bool.clone()], &engine);
    let lesser = Ty::new_effect_row([state_a, state_b], None, &engine);
    let greater = Ty::new_effect_row([state_int32, state_bool], None, &engine);

    let subst = solve(&mut solver, TyRelate::new(lesser, greater), &engine)
        .await
        .expect("same-constructor occurrences should match positionally");

    assert_eq!(subst.get(&a), Some(&int32));
    assert_eq!(subst.get(&b), Some(&bool));
}

// input: ?i = this, this = this, this = another trait's self, this = I
// premise: self is a rigid instance binder, in either equality direction
// output: inference binds to self; only identical self binders compare equal
#[tokio::test]
async fn self_instance_is_rigid_but_can_be_an_inference_solution() {
    use rayc_type::ty::self_instance::SelfInstance;
    let engine = rayc_qbice::create_minimal_engine().await;
    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let this = engine.intern(Ty::SelfInstance(SelfInstance::new(id(1))));
    for reverse in [false, true] {
        let mut solver = Solver::without_givens(engine.clone()).await;
        let inference = solver.new_inference(TyKind::Instance);
        let cases = [
            (
                engine.intern(Ty::Inference(inference)),
                Ok(Step::Subst(Subst::new_singleton(inference, this.clone()))),
            ),
            (this.clone(), Ok(Step::Derived(Vec::new()))),
            (engine.intern(Ty::SelfInstance(SelfInstance::new(id(2)))), Err(Error::Conflicted)),
            (Ty::new_instance(id(3), Args::new([], &engine), &engine), Err(Error::Conflicted)),
        ];
        for (other, expected) in cases {
            let (left, right) = if reverse { (other, this.clone()) } else { (this.clone(), other) };
            assert_eq!(solver.entail_ty_relate(&TyRelate::new(left, right)).await, expected);
        }
    }
}

// input: C.Item[List[int32]] = C.Item[Set[int32]]
// premise: Item is an opaque instance associated type
// output: NoProgress, without relating the distinct instance arguments
#[tokio::test]
async fn associated_types_are_not_structurally_decomposed() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let list = Ty::new_instance(id(1), Args::new([int32.clone()], &engine), &engine);
    let set = Ty::new_instance(id(2), Args::new([int32], &engine), &engine);
    let list_element = Ty::new_instance_associated(id(3), list, [], &engine);
    let set_element = Ty::new_instance_associated(id(3), set, [], &engine);
    let mut solver = Solver::without_givens(engine).await;

    let step = solver.entail_ty_relate(&TyRelate::new(list_element, set_element)).await;

    assert_eq!(step, Ok(Step::NoProgress));
}

// input: C.Item[List[int32]] = C.Item[List[int32]]
// premise: Item is an opaque instance associated type
// output: the syntactically identical constraint is discharged
#[tokio::test]
async fn syntactically_identical_associated_types_are_discharged() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let list = Ty::new_instance(id(1), Args::new([int32], &engine), &engine);
    let element = Ty::new_instance_associated(id(2), list, [], &engine);
    let mut solver = Solver::without_givens(engine).await;

    let step = solver.entail_ty_relate(&TyRelate::new(element.clone(), element)).await;

    assert_eq!(step, Ok(Step::Derived(Vec::new())));
}
