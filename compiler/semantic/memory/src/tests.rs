use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_arena::ID;
use rayc_ir::{
    address::{Address, Projection},
    cfg::{Block, Point},
    dataflow::{DataflowProblem, JoinLattice},
    ir_variable::IRVariableID,
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{
    Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine, create_minimal_engine,
};
use rayc_semantic_element::{
    parameter::ParameterID,
    struct_body::{Field, Key as StructBodyKey, StructBody},
};
use rayc_solver::Solver;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::{
    poly_var::{Key as PolyVarMapKey, PolyVarMap},
    ty::{Primitive, Ty, args::Args, self_instance::SelfInstance},
    where_clause::{AssociatedTypeEquality, PredicateKind},
};

use super::{PlaceState, PossibleStates, StackRoot, StackState, StackStateProblem};

fn point(instruction_idx: usize) -> Point {
    Point::builder().block_id(ID::<Block>::new(0)).instruction_idx(instruction_idx).build()
}

fn variable() -> StackRoot { StackRoot::Variable(IRVariableID::new(0)) }

fn leaf_type(engine: &TrackedEngine) -> Interned<Ty> { Ty::new_primitive(Primitive::Int32, engine) }

fn tuple_type(engine: &TrackedEngine) -> Interned<Ty> {
    let leaf = leaf_type(engine);
    Ty::new_tuple(engine.intern_unsized([leaf.clone(), leaf]), engine)
}

fn nested_tuple_type(engine: &TrackedEngine) -> Interned<Ty> {
    let leaf = leaf_type(engine);
    let nested = Ty::new_tuple(engine.intern_unsized([leaf.clone(), leaf.clone()]), engine);
    Ty::new_tuple(engine.intern_unsized([nested, leaf]), engine)
}

// input: move from component 0 of a type alias
// premise: the solver normalizes the alias to (int32, int32)
// output: component 0 is moved and component 1 remains initialized
#[tokio::test]
async fn projected_move_normalizes_the_type_before_inspecting_its_shape() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let alias = engine.intern(Ty::SelfInstance(SelfInstance::new(owner)));
    let normalized = tuple_type(&engine);
    let solver =
        Solver::with_givens(engine.clone(), owner, [PredicateKind::AssociatedTypeEquality(
            AssociatedTypeEquality::new(alias.clone(), normalized),
        )]);
    let mut problem = StackStateProblem::new(solver);
    problem.register(root, alias);
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());

    assert!(state.move_place(root, &[Projection::Tuple(0)], point(1), &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Partial(components) = slots.state(root).unwrap() else {
        panic!("the normalized tuple should be expanded");
    };
    assert!(!components[&Projection::Tuple(0)].is_initialized());
    assert!(components[&Projection::Tuple(1)].is_initialized());
}

// input: tuple projection from int32
// premise: this projection is invalid in type-checked IR
// output: internal compiler error
#[tokio::test]
#[should_panic(expected = "does not match normalized type")]
async fn projected_move_panics_when_the_projection_does_not_match_the_type() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, leaf_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());

    let _ = state.move_place(root, &[Projection::Tuple(0)], point(1), &problem).await;
}

fn test_span() -> RelativeSpan {
    RelativeSpan {
        start: RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID },
        end: RelativeLocation { offset: 1, mode: OffsetMode::End, relative_to: ROOT_BRANCH_ID },
        source_id: GlobalSourceID::default(),
    }
}

async fn recursive_struct() -> (TrackedEngine, Interned<Ty>, Projection) {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let struct_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        PolyVarMapKey { symbol_id: struct_id },
        engine.intern(PolyVarMap::new()),
    )]))));

    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let recursive_ty = Ty::new_struct(struct_id, Args::new([], &tracked), &tracked);
    let mut body = StructBody::new();
    let field_id = body
        .insert(
            Field::builder()
                .name(tracked.intern_unsized("tail"))
                .span(test_span())
                .ty(recursive_ty.clone())
                .build(),
        )
        .unwrap();
    let body = tracked.intern(body);
    drop(tracked);

    Arc::get_mut(&mut engine).unwrap().register_executor(Arc::new(PrecomputedExecutor::new(
        HashMap::from([(StructBodyKey { symbol_id: struct_id }, body)]),
    )));

    (engine.tracked().await, recursive_ty, Projection::Field(field_id))
}

#[tokio::test]
async fn move_rejects_a_never_initialized_place() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, leaf_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::uninitialized());
    let original = state.clone();

    assert!(!state.move_place(root, &[], point(1), &problem).await);
    assert_eq!(state, original);
}

#[tokio::test]
async fn boundary_initializes_parameters_but_not_variables() {
    let variable = variable();
    let parameter = StackRoot::Parameter(ParameterID::new(0));
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(variable, leaf_type(&engine));
    problem.register(parameter, leaf_type(&engine));

    let boundary = problem.boundary_facts(ID::<Block>::new(0)).await.unwrap();

    let StackState::Reachable(slots) = boundary else {
        panic!("boundary facts should be reachable");
    };
    assert!(!slots.state(variable).unwrap().is_initialized());
    assert!(slots.state(parameter).unwrap().is_initialized());
}

#[tokio::test]
async fn move_rejects_a_place_after_its_first_move() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, leaf_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());

    assert!(state.move_place(root, &[], point(1), &problem).await);
    assert!(!state.move_place(root, &[], point(2), &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Uniform(PossibleStates::Uninitialized(history)) = slots.state(root).unwrap()
    else {
        panic!("the moved place should be uniformly uninitialized");
    };
    assert_eq!(history.points().collect::<Vec<_>>(), vec![point(1)]);
}

#[tokio::test]
async fn moving_one_component_partially_initializes_its_aggregate() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, tuple_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());

    assert!(state.move_place(root, &[Projection::Tuple(0)], point(1), &problem).await);
    assert!(!state.move_place(root, &[], point(2), &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Partial(components) = slots.state(root).unwrap() else {
        panic!("moving a tuple component should expand its aggregate state");
    };
    assert!(!components[&Projection::Tuple(0)].is_initialized());
    assert!(components[&Projection::Tuple(1)].is_initialized());
}

#[tokio::test]
async fn moving_an_initialized_sibling_through_a_partial_parent_succeeds() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, tuple_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());
    assert!(state.move_place(root, &[Projection::Tuple(0)], point(1), &problem).await);

    assert!(state.move_place(root, &[Projection::Tuple(1)], point(2), &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Partial(components) = slots.state(root).unwrap() else {
        panic!("moving tuple components should preserve their independent states");
    };
    assert!(!components[&Projection::Tuple(0)].is_initialized());
    assert!(!components[&Projection::Tuple(1)].is_initialized());
}

#[tokio::test]
async fn restoring_a_nested_place_preserves_uninitialized_siblings() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, nested_tuple_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::uninitialized());
    let mut address = Address::new_variable(IRVariableID::new(0), &engine);
    address.add_tuple_index(0, &engine);
    address.add_tuple_index(1, &engine);

    assert!(state.restore(&address, &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Partial(root_components) = slots.state(root).unwrap() else {
        panic!("restoring a nested component should expand the root tuple");
    };
    let PlaceState::Partial(nested_components) = &root_components[&Projection::Tuple(0)] else {
        panic!("restoring a nested component should expand its parent tuple");
    };
    assert!(!nested_components[&Projection::Tuple(0)].is_initialized());
    assert!(nested_components[&Projection::Tuple(1)].is_initialized());
    assert!(!root_components[&Projection::Tuple(1)].is_initialized());
}

#[tokio::test]
async fn restoring_the_last_moved_component_reinitializes_the_aggregate() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, tuple_type(&engine));
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());
    let projections = [Projection::Tuple(0)];
    assert!(state.move_place(root, &projections, point(1), &problem).await);

    assert!(state.restore_place(root, &projections, &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    assert_eq!(slots.state(root), Some(&PlaceState::initialized()));
}

#[tokio::test]
async fn projected_move_expands_recursive_structs_only_along_the_address() {
    let root = variable();
    let (engine, recursive_ty, field) = recursive_struct().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine));
    problem.register(root, recursive_ty);
    let mut state = StackState::reachable();
    let StackState::Reachable(slots) = &mut state else {
        unreachable!();
    };
    slots.set(root, PlaceState::initialized());

    assert!(state.move_place(root, &[field, field], point(1), &problem).await);

    let StackState::Reachable(slots) = state else {
        unreachable!();
    };
    let PlaceState::Partial(root_components) = slots.state(root).unwrap() else {
        panic!("the root recursive aggregate should be expanded once");
    };
    let PlaceState::Partial(nested_components) = &root_components[&field] else {
        panic!("the projected recursive child should be expanded once");
    };
    assert!(!nested_components[&field].is_initialized());
}

#[tokio::test]
async fn join_preserves_partial_initialization() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, tuple_type(&engine));

    let mut left_slots = super::StackSlots::new();
    left_slots.set(root, PlaceState::initialized());
    let mut left = StackState::Reachable(left_slots);
    assert!(left.move_place(root, &[Projection::Tuple(0)], point(1), &problem).await);
    let mut right_slots = super::StackSlots::new();
    right_slots.set(root, PlaceState::initialized());
    let right = StackState::Reachable(right_slots);

    assert!(!left.join(&right, &problem).await.unwrap());

    let StackState::Reachable(slots) = left else {
        unreachable!();
    };
    let PlaceState::Partial(components) = slots.state(root).unwrap() else {
        panic!("the moved component should remain uninitialized after the join");
    };
    assert!(!components[&Projection::Tuple(0)].is_initialized());
    assert!(components[&Projection::Tuple(1)].is_initialized());
}

#[tokio::test]
async fn join_preserves_each_possible_last_move() {
    let root = variable();
    let engine = create_minimal_engine().await;
    let mut problem = StackStateProblem::new(Solver::without_givens(engine.clone()));
    problem.register(root, leaf_type(&engine));

    let mut left_slots = super::StackSlots::new();
    left_slots.set(root, PlaceState::moved_at(point(1)));
    let mut left = StackState::Reachable(left_slots);
    let mut right_slots = super::StackSlots::new();
    right_slots.set(root, PlaceState::moved_at(point(2)));
    let right = StackState::Reachable(right_slots);

    assert!(left.join(&right, &problem).await.unwrap());

    let StackState::Reachable(slots) = left else {
        unreachable!();
    };
    let PlaceState::Uniform(PossibleStates::Uninitialized(history)) = slots.state(root).unwrap()
    else {
        panic!("joining moved places should produce an uninitialized place");
    };
    assert_eq!(history.points().collect::<Vec<_>>(), vec![point(1), point(2)]);
    assert!(!history.may_be_uninitialized_without_move());
}
