use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;
use rayc_type::{
    constraint::outlives::{OutlivesConstraint, OutlivesConstraints},
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    subst::{Subst, Substitutable},
    ty::{
        Mutability, Primitive, Ty, TyKind, args::Args, effect_row::EffectLabel,
        inference::Inference, lifetime::Lifetime,
    },
    variance::{Variance, VarianceKey, VarianceMap},
};

use super::{Solver, TyRelate};
use crate::ty_relate::{DerivationRule, DerivedConstraint, Entailment, Error, Step};

/// Takes one step towards solving `relate`, returning the step and the
/// outlives constraints it requires.
async fn entail(
    solver: &mut Solver,
    relate: &TyRelate,
) -> Result<(Step, OutlivesConstraints), Error> {
    solver.entail_ty_relate(relate).await.map(Entailment::into_parts)
}

/// Takes one step towards solving `relate`, which requires no outlives
/// constraint, returning the step.
async fn entail_step(solver: &mut Solver, relate: &TyRelate) -> Result<Step, Error> {
    let (step, outlives) = entail(solver, relate).await?;
    assert!(outlives.is_empty(), "the step should require no outlives constraint");
    Ok(step)
}

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

/// Creates an engine that knows `members` as associated types of kind
/// `Star`.
async fn engine_with_associated_types(
    members: impl IntoIterator<Item = GlobalSymbolID>,
) -> TrackedEngine {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(
        members
            .into_iter()
            .map(|symbol_id| (rayc_type::associated_type_kind::Key { symbol_id }, TyKind::Star))
            .collect::<HashMap<_, _>>(),
    )));
    Arc::new(engine).tracked().await
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
            entail_step(&mut solver, &TyRelate::new_invariant(left, right)).await,
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
            entail_step(&mut solver, &TyRelate::new_invariant(variable, error.clone())).await,
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
                    entail_step(&mut solver, &TyRelate::new_invariant(left, right)).await,
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
        assert_eq!(
            entail_step(&mut solver, &TyRelate::new_invariant(variable.clone(), ty)).await,
            expected
        );
        for rejected in [Ty::new_unit(&engine), Ty::new_star_error(&engine)] {
            assert_eq!(
                entail_step(&mut solver, &TyRelate::new_invariant(variable.clone(), rejected))
                    .await,
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
        match entail_step(solver, &constraint).await? {
            Step::Subst(new_subst) => subst.compose(&new_subst, engine),
            Step::Generalized { subst: new_subst, derived } => {
                subst.compose(&new_subst, engine);
                pending.extend(derived.into_iter().map(|x| x.ty_relate));
            }
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

    let step = entail_step(&mut solver, &TyRelate::new_invariant(lesser, greater)).await;

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

    let step = entail_step(&mut solver, &(TyRelate::new_invariant(lesser, greater))).await;

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

    let step = entail_step(&mut solver, &(TyRelate::new_invariant(lesser, greater))).await;

    let e3 = engine.intern(Ty::Inference(Inference::new(TyKind::EffectRow, 2)));
    let state_remainder = Ty::new_effect_row([state], Some(e3.clone()), &engine);
    let io_remainder = Ty::new_effect_row([io], Some(e3), &engine);
    assert_eq!(
        step,
        Ok(Step::Derived(vec![
            DerivedConstraint::new_type_application_matching(
                e1,
                state_remainder,
                Variance::Invariant
            ),
            DerivedConstraint::new_type_application_matching(io_remainder, e2, Variance::Invariant),
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

    let subst = solve(&mut solver, TyRelate::new_invariant(lesser, greater), &engine)
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

    let subst = solve(&mut solver, TyRelate::new_invariant(open, closed), &engine)
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

    let subst = solve(&mut solver, TyRelate::new_invariant(open, duplicate), &engine)
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

    let result = solve(&mut solver, TyRelate::new_invariant(e_ty, recursive_row), &engine).await;

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

    let subst = solve(&mut solver, TyRelate::new_invariant(poly_ty.clone(), e_ty), &engine)
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

    let subst = solve(&mut solver, TyRelate::new_invariant(rigid_row, inferred_row), &engine)
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

    let step =
        entail_step(&mut solver, &(TyRelate::new_invariant(inference_ty, row.clone()))).await;

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

    let result = solve(&mut solver, TyRelate::new_invariant(lesser, greater), &engine).await;

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

    let subst = solve(&mut solver, TyRelate::new_invariant(lesser, greater), &engine)
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
            assert_eq!(
                entail_step(&mut solver, &TyRelate::new_invariant(left, right)).await,
                expected
            );
        }
    }
}

// input: this.Item[int32] = this.Item[bool]
// premise: Item is an opaque associated type with no given
// output: NoProgress, without relating the distinct arguments
#[tokio::test]
async fn associated_types_are_not_structurally_decomposed() {
    use rayc_type::ty::self_instance::SelfInstance;

    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let engine = engine_with_associated_types([id(2)]).await;
    let this = engine.intern(Ty::SelfInstance(SelfInstance::new(id(1))));
    let item = |argument| Ty::new_instance_associated(id(2), this.clone(), [argument], &engine);
    let int_item = item(Ty::new_primitive(Primitive::Int32, &engine));
    let bool_item = item(Ty::new_primitive(Primitive::Bool, &engine));
    let mut solver = Solver::without_givens(engine.clone()).await;

    let step = entail_step(&mut solver, &TyRelate::new_invariant(int_item, bool_item)).await;

    assert_eq!(step, Ok(Step::NoProgress));
}

// input: this.Item[int32] = this.Item[int32]
// premise: Item is an opaque associated type with no given
// output: the syntactically identical constraint is discharged
#[tokio::test]
async fn syntactically_identical_associated_types_are_discharged() {
    use rayc_type::ty::self_instance::SelfInstance;

    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let engine = engine_with_associated_types([id(2)]).await;
    let this = engine.intern(Ty::SelfInstance(SelfInstance::new(id(1))));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let element = Ty::new_instance_associated(id(2), this, [int32], &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;

    let step = entail_step(&mut solver, &TyRelate::new_invariant(element.clone(), element)).await;

    assert_eq!(step, Ok(Step::Derived(Vec::new())));
}

fn region(index: u64, engine: &TrackedEngine) -> Interned<Ty> {
    Ty::new_lifetime(Lifetime::Region(rayc_arena::ID::new(index)), engine)
}

fn outlives_of(lesser: &Interned<Ty>, greater: &Interned<Ty>) -> OutlivesConstraint {
    OutlivesConstraint::new(lesser.clone(), greater.clone())
}

fn constraints<const N: usize>(constraints: [OutlivesConstraint; N]) -> OutlivesConstraints {
    constraints.into_iter().collect()
}

/// Solves `constraint` to completion, returning the substitution and every
/// outlives constraint produced along the way.
async fn solve_with_outlives(
    solver: &mut Solver,
    constraint: TyRelate,
    engine: &TrackedEngine,
) -> Result<(Subst, OutlivesConstraints), Error> {
    let mut pending = vec![constraint];
    let mut subst = Subst::new_empty();
    let mut outlives = OutlivesConstraints::new();

    while let Some(constraint) = pending.pop() {
        let constraint = constraint.apply_subst_or_clone(&subst, engine);
        let (step, new_outlives) = entail(solver, &constraint).await?;
        outlives = outlives.union(new_outlives);
        match step {
            Step::Subst(new_subst) => subst.compose(&new_subst, engine),
            Step::Generalized { subst: new_subst, derived } => {
                subst.compose(&new_subst, engine);
                pending.extend(derived.into_iter().map(|x| x.ty_relate));
            }
            Step::Derived(derived) => pending.extend(derived.into_iter().map(|x| x.ty_relate)),
            Step::NoProgress => panic!("the relation should make progress"),
        }
    }

    Ok((subst, outlives))
}

/// Creates an engine with one effect whose single lifetime parameter has the
/// given variance.
async fn engine_with_lifetime_effect(variance: Variance) -> (TrackedEngine, GlobalSymbolID) {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let effect_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
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
    let mut poly_vars = PolyVarMap::new();
    poly_vars
        .insert(
            PolyVar::new_lifetime(engine.intern_unsized("'a"), span)
                .with_declared_variance(variance),
        )
        .unwrap();
    let variances = engine.intern(VarianceMap::new(&poly_vars));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        VarianceKey { symbol_id: effect_id },
        variances,
    )]))));
    (Arc::new(engine).tracked().await, effect_id)
}

// input: '?0 R '?1 for each variance R
// premise: the solver keeps outlives constraints
// output: covariant '?0: '?1, contravariant '?1: '?0, invariant both,
//         bivariant nothing; no substitution in any case
#[tokio::test]
async fn lifetimes_relate_by_outlives_constraints_following_the_variance() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let (a, b) = (region(0, &engine), region(1, &engine));
    for (variance, expected) in [
        (Variance::Covariant, constraints([outlives_of(&a, &b)])),
        (Variance::Contravariant, constraints([outlives_of(&b, &a)])),
        (Variance::Invariant, constraints([outlives_of(&a, &b), outlives_of(&b, &a)])),
        (Variance::Bivariant, OutlivesConstraints::new()),
    ] {
        let mut solver = Solver::without_givens(engine.clone()).await;
        let step = entail(&mut solver, &TyRelate::new(a.clone(), b.clone(), variance)).await;

        assert_eq!(step, Ok((Step::Derived(Vec::new()), expected)));
    }
}

// input: '_ = '?0, 'static = '?0, '?0 = '?0
// premise: the solver keeps outlives constraints
// output: only '?0: 'static; erased lifetimes and constraints that always hold
//         are left out
#[tokio::test]
async fn outlives_constraints_that_always_hold_are_left_out() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let a = region(0, &engine);
    let erased = Ty::new_lifetime(Lifetime::Erased, &engine);
    let static_ = Ty::new_lifetime(Lifetime::Static, &engine);
    let mut solver = Solver::without_givens(engine.clone()).await;

    let erased_step = entail(&mut solver, &TyRelate::new_invariant(erased, a.clone())).await;
    let static_step =
        entail(&mut solver, &TyRelate::new_invariant(static_.clone(), a.clone())).await;

    assert_eq!(erased_step, Ok((Step::Derived(Vec::new()), OutlivesConstraints::new())));
    assert_eq!(
        static_step,
        Ok((Step::Derived(Vec::new()), constraints([outlives_of(&a, &static_)])))
    );
}

// input: &'?0 mut &'?1 int32 <: &'?2 mut &'?3 int32
// premise: a mutable reference is covariant in its lifetime and invariant in
//          its pointee
// output: '?0: '?2, '?1: '?3, '?3: '?1
#[tokio::test]
async fn mutable_reference_relates_its_pointee_invariantly() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let r = |outer, inner| {
        let pointee = Ty::new_reference(
            region(inner, &engine),
            int32.clone(),
            Mutability::Immutable,
            &engine,
        );
        Ty::new_reference(region(outer, &engine), pointee, Mutability::Mutable, &engine)
    };
    let mut solver = Solver::without_givens(engine.clone()).await;

    let (subst, outlives) = solve_with_outlives(
        &mut solver,
        TyRelate::new(r(0, 1), r(2, 3), Variance::Covariant),
        &engine,
    )
    .await
    .unwrap();

    let (r0, r1, r2, r3) =
        (region(0, &engine), region(1, &engine), region(2, &engine), region(3, &engine));
    assert!(subst.is_empty());
    assert_eq!(
        outlives,
        constraints([outlives_of(&r0, &r2), outlives_of(&r1, &r3), outlives_of(&r3, &r1)])
    );
}

// input: {Reader['?0]} <: {Reader['?1]} and {Log['?0]} <: {Log['?1]}
// premise: Reader's lifetime is contravariant and Log's is covariant
// output: '?1: '?0 for Reader, '?0: '?1 for Log
#[tokio::test]
async fn effect_label_arguments_relate_by_the_effect_variance() {
    for (variance, flipped) in [(Variance::Contravariant, true), (Variance::Covariant, false)] {
        let (engine, effect_id) = engine_with_lifetime_effect(variance).await;
        let (a, b) = (region(0, &engine), region(1, &engine));
        let row = |lifetime: &Interned<Ty>| {
            let label = EffectLabel::new(effect_id, Args::new([lifetime.clone()], &engine));
            Ty::new_effect_row([engine.intern(label)], None, &engine)
        };
        let mut solver = Solver::without_givens(engine.clone()).await;

        let (_, outlives) = solve_with_outlives(
            &mut solver,
            TyRelate::new(row(&a), row(&b), Variance::Covariant),
            &engine,
        )
        .await
        .unwrap();

        let expected = if flipped { outlives_of(&b, &a) } else { outlives_of(&a, &b) };
        assert_eq!(outlives, constraints([expected]));
    }
}

// input: ?t <: (&'?0 int32, ?u)
// premise: ?t and ?u are unconstrained type inference variables
// output: ?t := (&?'l int32, ?v) for a fresh lifetime ?'l and a fresh ?v,
//         then (&?'l int32, ?v) <: (&'?0 int32, ?u)
#[tokio::test]
async fn covariant_binding_generalizes_the_other_side() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let t = solver.new_inference(TyKind::Star);
    let u = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let reference =
        |lifetime| Ty::new_reference(lifetime, int32.clone(), Mutability::Immutable, &engine);
    let tuple = |first, second| Ty::new_tuple(engine.intern_unsized([first, second]), &engine);
    let target = tuple(reference(region(0, &engine)), u);

    let step = entail_step(
        &mut solver,
        &TyRelate::new(engine.intern(Ty::Inference(t)), target.clone(), Variance::Covariant),
    )
    .await;

    let l = engine.intern(Ty::Inference(Inference::new(TyKind::Lifetime, 2)));
    let v = engine.intern(Ty::Inference(Inference::new(TyKind::Star, 3)));
    let generalized = tuple(reference(l), v);
    assert_eq!(
        step,
        Ok(Step::Generalized {
            subst: Subst::new_singleton(t, generalized.clone()),
            derived: vec![DerivedConstraint::new(
                DerivationRule::Generalization,
                TyRelate::new(generalized, target, Variance::Covariant),
            )],
        })
    );
}

// input: ?t = (&'?0 int32, ?u)
// premise: ?t and ?u are unconstrained type inference variables
// output: ?t := (&'?0 int32, ?u), without generalization
#[tokio::test]
async fn invariant_binding_does_not_generalize() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let t = solver.new_inference(TyKind::Star);
    let u = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let reference = Ty::new_reference(region(0, &engine), int32, Mutability::Immutable, &engine);
    let target = Ty::new_tuple(engine.intern_unsized([reference, u]), &engine);

    let step = entail_step(
        &mut solver,
        &TyRelate::new_invariant(engine.intern(Ty::Inference(t)), target.clone()),
    )
    .await;

    assert_eq!(step, Ok(Step::Subst(Subst::new_singleton(t, target))));
}

// input: ?t <: (?t,)
// premise: ?t is a type inference variable
// output: OccursCheckFailed, from generalization
#[tokio::test]
async fn generalization_runs_the_occurs_check() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let t = engine.intern(Ty::Inference(solver.new_inference(TyKind::Star)));
    let tuple = Ty::new_tuple(engine.intern_unsized([t.clone()]), &engine);

    let step = entail_step(&mut solver, &TyRelate::new(t, tuple, Variance::Covariant)).await;

    assert_eq!(step, Err(Error::OccursCheckFailed));
}

// input: this.Out['?0] = this.Out['?1]
// premise: no given reduces this.Out
// output: '?0: '?1, '?1: '?0, since projection arguments are invariant
#[tokio::test]
async fn rigid_projections_differing_in_lifetimes_relate_them_invariantly() {
    use rayc_type::ty::self_instance::SelfInstance;

    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let engine = engine_with_associated_types([id(2)]).await;
    let this = engine.intern(Ty::SelfInstance(SelfInstance::new(id(1))));
    let (a, b) = (region(0, &engine), region(1, &engine));
    let out = |lifetime: &Interned<Ty>| {
        Ty::new_instance_associated(id(2), this.clone(), [lifetime.clone()], &engine)
    };
    let mut solver = Solver::without_givens(engine.clone()).await;

    let step = entail(&mut solver, &TyRelate::new(out(&a), out(&b), Variance::Covariant)).await;

    assert_eq!(
        step,
        Ok((Step::Derived(Vec::new()), constraints([outlives_of(&a, &b), outlives_of(&b, &a)])))
    );
}

// input: ?'l = '?0
// premise: ?'l is a lifetime inference variable
// output: no substitution; ?'l: '?0 and '?0: ?'l
#[tokio::test]
async fn lifetime_inference_is_never_bound() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let var_ty = engine.intern(Ty::Inference(solver.new_inference(TyKind::Lifetime)));
    let a = region(0, &engine);

    let step = entail(&mut solver, &TyRelate::new_invariant(var_ty.clone(), a.clone())).await;

    assert_eq!(
        step,
        Ok((
            Step::Derived(Vec::new()),
            constraints([outlives_of(&var_ty, &a), outlives_of(&a, &var_ty)])
        ))
    );
}

// input: ?t <: ?u, then ?t <: ?n
// premise: ?t and ?u are unconstrained; ?n is a numeric literal
// output: ?t <: ?u waits for a binding; ?t <: ?n unifies the variables, since
//         ?n can only stand for a primitive type without lifetimes
#[tokio::test]
async fn subtyping_between_variables_waits_unless_one_is_lifetime_free() {
    use rayc_type::ty::InferenceConstraint;

    let engine = rayc_qbice::create_minimal_engine().await;
    let mut solver = Solver::without_givens(engine.clone()).await;
    let t = solver.new_inference(TyKind::Star);
    let u = solver.new_inference(TyKind::Star);
    let n = solver.new_inference_with_constraint(TyKind::Star, InferenceConstraint::Numeric);
    let [t_ty, u_ty, n_ty] = [t, u, n].map(|var| engine.intern(Ty::Inference(var)));

    let waiting =
        entail_step(&mut solver, &TyRelate::new(t_ty.clone(), u_ty, Variance::Covariant)).await;
    let unified =
        entail_step(&mut solver, &TyRelate::new(t_ty, n_ty.clone(), Variance::Covariant)).await;

    assert_eq!(waiting, Ok(Step::NoProgress));
    let common = engine.intern(Ty::Inference(Inference::new_with_constraint(
        TyKind::Star,
        InferenceConstraint::Numeric,
        3,
    )));
    assert_eq!(unified, Ok(Step::Subst([(t, common.clone()), (n, common)].into_iter().collect())));
}
