use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_arena::ID;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_semantic_element::{
    all_instance_implements_trait::AllInstanceImplementsTrait,
    instance_trait_ref::Key as InstanceTraitRefKey,
};
use rayc_source_file::LocalSourceID;
use rayc_symbol::{GlobalSymbolID, MemberID, SymbolID};
use rayc_target::{Global, TargetID};
use rayc_type::{
    poly_var::{
        EnclosingMapsKey, GlobalPolyVarID, Key as PolyVarKey, PolyVar, PolyVarMap, PolyVarStack,
    },
    solver::Solver,
    trait_ref::TraitRef,
    ty::{Primitive, Ty, TyKind, args::Args},
};

use super::{
    EnteredInstanceGoal, InstanceResolutionEdge, InstanceResolutionError, InstanceResolutionLimit,
    InstanceResolutionLimits, InstanceResolutionState, InstanceResolutionStateError,
    InstanceResolver,
};

#[derive(Debug)]
enum InstanceCandidateParameter {
    Ordinary { id: GlobalPolyVarID, kind: TyKind },
    Given { id: GlobalPolyVarID, required: TraitRef },
}

impl InstanceCandidateParameter {
    const fn ordinary(id: GlobalPolyVarID, kind: TyKind) -> Self { Self::Ordinary { id, kind } }

    const fn given(id: GlobalPolyVarID, required: TraitRef) -> Self { Self::Given { id, required } }
}

#[derive(Debug)]
struct InstanceCandidate {
    instance_id: GlobalSymbolID,
    head: TraitRef,
    parameters: Vec<InstanceCandidateParameter>,
}

impl InstanceCandidate {
    const fn new(
        instance_id: GlobalSymbolID,
        head: TraitRef,
        parameters: Vec<InstanceCandidateParameter>,
    ) -> Self {
        Self { instance_id, head, parameters }
    }

    const fn instance_id(&self) -> GlobalSymbolID { self.instance_id }

    const fn head(&self) -> &TraitRef { &self.head }

    fn parameters(&self) -> &[InstanceCandidateParameter] { &self.parameters }
}

fn symbol(id: u128) -> Global<SymbolID> { TargetID::TEST.make_global(SymbolID::from_u128(id)) }

fn site() -> GlobalSymbolID { symbol(1_000) }

async fn goal(id: u128) -> TraitRef {
    let engine = rayc_qbice::create_minimal_engine().await;
    TraitRef::new(symbol(id), Args::new([], &engine))
}

fn edge(id: u128) -> InstanceResolutionEdge {
    InstanceResolutionEdge::new(symbol(id), MemberID::new(symbol(id), ID::<PolyVar>::new(0)))
}

fn trait_ref(
    trait_id: u128,
    args: impl IntoIterator<Item = Interned<Ty>>,
    engine: &TrackedEngine,
) -> TraitRef {
    TraitRef::new(symbol(trait_id), Args::new(args, engine))
}

fn given_id(instance_id: u128, index: u64) -> GlobalPolyVarID {
    GlobalPolyVarID::new(symbol(instance_id), ID::new(index))
}

fn test_span() -> RelativeSpan {
    let location = RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::new(0) };
    RelativeSpan {
        start: location,
        end: location,
        source_id: TargetID::TEST.make_global(LocalSourceID::new(0, 0)),
    }
}

async fn engine_with_candidates(candidates: &[InstanceCandidate]) -> TrackedEngine {
    engine_with_candidates_and_lexical(candidates, &[]).await
}

async fn engine_with_candidates_and_lexical(
    candidates: &[InstanceCandidate],
    lexical_requirements: &[TraitRef],
) -> TrackedEngine {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();

    let mut ids_by_trait = FxHashMap::<GlobalSymbolID, Vec<GlobalSymbolID>>::default();
    let mut heads = HashMap::new();
    let mut parameter_maps = HashMap::new();
    for candidate in candidates {
        ids_by_trait.entry(candidate.head().trait_id()).or_default().push(candidate.instance_id());
        heads.insert(
            InstanceTraitRefKey { symbol_id: candidate.instance_id() },
            Some(candidate.head().clone()),
        );

        let mut parameter_map = PolyVarMap::new();
        for (index, parameter) in candidate.parameters().iter().enumerate() {
            let (expected_id, parameter) = match parameter {
                InstanceCandidateParameter::Ordinary { id, kind } => {
                    let name = engine.intern_unsized(format!("p{index}"));
                    let parameter = match kind {
                        TyKind::Star => PolyVar::new_type(name, test_span()),
                        TyKind::EffectRow => PolyVar::new_effect(name, test_span()),
                        TyKind::Instance => {
                            panic!("an ordinary candidate parameter cannot have instance kind")
                        }
                    };
                    (*id, parameter)
                }
                InstanceCandidateParameter::Given { id, required } => (
                    *id,
                    PolyVar::new_instance(
                        engine.intern_unsized(format!("p{index}")),
                        required.clone(),
                        test_span(),
                    ),
                ),
            };
            let actual_id = parameter_map.insert(parameter).unwrap();
            assert_eq!(GlobalPolyVarID::new(candidate.instance_id(), actual_id), expected_id);
        }
        parameter_maps.insert(
            PolyVarKey { symbol_id: candidate.instance_id() },
            engine.intern(parameter_map),
        );
    }

    let instance_lists = ids_by_trait
        .into_iter()
        .map(|(trait_id, ids)| {
            (
                AllInstanceImplementsTrait { trait_id, target_id: TargetID::TEST },
                engine.intern_unsized(ids),
            )
        })
        .collect::<HashMap<_, _>>();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(instance_lists)));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(heads)));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(parameter_maps)));
    let lexical_scope = lexical_scope(&engine, symbol(20), lexical_requirements.iter().cloned());
    let lexical_scope = engine.intern(lexical_scope);
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        EnclosingMapsKey { symbol_id: site() },
        lexical_scope,
    )]))));
    Arc::new(engine).tracked().await
}

fn lexical_scope(
    engine: &Engine,
    owner: GlobalSymbolID,
    requirements: impl IntoIterator<Item = TraitRef>,
) -> PolyVarStack {
    let mut map = PolyVarMap::new();
    for (index, required) in requirements.into_iter().enumerate() {
        map.insert(PolyVar::new_instance(
            engine.intern_unsized(format!("d{index}")),
            required,
            test_span(),
        ))
        .unwrap();
    }
    let mut stack = PolyVarStack::new();
    stack.push(owner, engine.intern(map));
    stack
}

// input: enter A, then B, then A
// premise: B was introduced by a candidate premise while A remains active
// output: Cycle with path A -> B -> A
#[tokio::test]
async fn reentering_an_active_goal_reports_the_complete_cycle() {
    let a = goal(1).await;
    let b = goal(2).await;
    let mut state = InstanceResolutionState::default();
    let EnteredInstanceGoal::Active(a_active) = state.enter_goal(a.clone(), None).unwrap() else {
        panic!("a new goal should be active");
    };
    let EnteredInstanceGoal::Active(b_active) =
        state.enter_goal(b.clone(), Some(edge(10))).unwrap()
    else {
        panic!("a new goal should be active");
    };

    let InstanceResolutionStateError::Cycle(cycle) =
        state.enter_goal(a.clone(), Some(edge(11))).unwrap_err()
    else {
        panic!("reentering an active goal should report a cycle");
    };
    assert_eq!(
        cycle.path().iter().map(super::InstanceResolutionFrame::goal).collect::<Vec<_>>(),
        vec![&a, &b, &a]
    );

    state.leave_goal(b_active);
    state.leave_goal(a_active);
}

// input: A -> B -> C -> B, followed by completions for C and B
// premise: only frames descended from the first B depend on the cycle cut
// output: C is not memoized while the original B is memoized
#[tokio::test]
async fn cycles_disable_memoization_only_above_the_repeated_goal() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let a = goal(1).await;
    let b = goal(2).await;
    let c = goal(3).await;
    let resolved = Ty::new_instance(symbol(10), Args::new([], &engine), &engine);
    let mut state = InstanceResolutionState::default();
    let EnteredInstanceGoal::Active(a_active) = state.enter_goal(a, None).unwrap() else {
        panic!("a new goal should be active");
    };
    let EnteredInstanceGoal::Active(b_active) =
        state.enter_goal(b.clone(), Some(edge(10))).unwrap()
    else {
        panic!("a new goal should be active");
    };
    let EnteredInstanceGoal::Active(c_active) =
        state.enter_goal(c.clone(), Some(edge(11))).unwrap()
    else {
        panic!("a new goal should be active");
    };

    assert!(matches!(
        state.enter_goal(b.clone(), Some(edge(12))),
        Err(InstanceResolutionStateError::Cycle(_))
    ));
    state.complete_goal(c_active, Ok(resolved.clone()));
    state.complete_goal(b_active, Ok(resolved.clone()));
    state.leave_goal(a_active);

    let EnteredInstanceGoal::Active(c_active) = state.enter_goal(c, None).unwrap() else {
        panic!("the cycle-dependent descendant should not be memoized");
    };
    state.leave_goal(c_active);
    assert_eq!(state.enter_goal(b, None), Ok(EnteredInstanceGoal::Memoized(Ok(resolved))));
}

// input: B -> B followed by completion of the original B with the cycle error
// premise: the original occurrence is the root of the cyclic search
// output: the propagated cycle error is memoized for B
#[tokio::test]
async fn cycle_errors_are_memoized_at_the_original_occurrence() {
    let b = goal(1).await;
    let mut state = InstanceResolutionState::default();
    let EnteredInstanceGoal::Active(active) = state.enter_goal(b.clone(), None).unwrap() else {
        panic!("a new goal should be active");
    };
    let error =
        InstanceResolutionError::from(state.enter_goal(b.clone(), Some(edge(10))).unwrap_err());

    state.complete_goal(active, Err(error.clone()));

    assert_eq!(state.enter_goal(b, None), Ok(EnteredInstanceGoal::Memoized(Err(error))));
}

// input: three candidate visits split across a root and nested goal
// premise: the root budget permits two visits
// output: CandidateVisits limit on the third visit
#[tokio::test]
async fn candidate_fuel_is_shared_by_recursive_branches() {
    let mut state = InstanceResolutionState::new(InstanceResolutionLimits::new(4, 2));
    let EnteredInstanceGoal::Active(root) = state.enter_goal(goal(1).await, None).unwrap() else {
        panic!("a new goal should be active");
    };
    state.visit_candidate(symbol(10)).unwrap();
    let EnteredInstanceGoal::Active(nested) =
        state.enter_goal(goal(2).await, Some(edge(10))).unwrap()
    else {
        panic!("a new goal should be active");
    };
    state.visit_candidate(symbol(11)).unwrap();

    assert!(matches!(
        state.visit_candidate(symbol(12)),
        Err(InstanceResolutionStateError::Limit {
            limit: InstanceResolutionLimit::CandidateVisits { limit: 2, candidate },
            ..
        }) if candidate == symbol(12)
    ));

    state.leave_goal(nested);
    state.leave_goal(root);
}

// input: fuel exhaustion with two active goals, followed by successful
// completions premise: a limit makes every active result dependent on an
// incomplete search output: neither active goal is memoized
#[tokio::test]
async fn fuel_exhaustion_disables_memoization_for_the_active_stack() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let a = goal(1).await;
    let b = goal(2).await;
    let resolved = Ty::new_instance(symbol(10), Args::new([], &engine), &engine);
    let mut state = InstanceResolutionState::new(InstanceResolutionLimits::new(2, 0));
    let EnteredInstanceGoal::Active(a_active) = state.enter_goal(a.clone(), None).unwrap() else {
        panic!("a new goal should be active");
    };
    let EnteredInstanceGoal::Active(b_active) =
        state.enter_goal(b.clone(), Some(edge(10))).unwrap()
    else {
        panic!("a new goal should be active");
    };

    assert!(matches!(
        state.visit_candidate(symbol(11)),
        Err(InstanceResolutionStateError::Limit {
            limit: InstanceResolutionLimit::CandidateVisits { .. },
            ..
        })
    ));
    state.complete_goal(b_active, Ok(resolved.clone()));
    state.complete_goal(a_active, Ok(resolved));

    let EnteredInstanceGoal::Active(a_active) = state.enter_goal(a, None).unwrap() else {
        panic!("the limited root should not be memoized");
    };
    state.leave_goal(a_active);
    let EnteredInstanceGoal::Active(b_active) = state.enter_goal(b, None).unwrap() else {
        panic!("the limited descendant should not be memoized");
    };
    state.leave_goal(b_active);
}

// input: enter a nested goal beneath one active root
// premise: the maximum depth is one
// output: Depth limit before the nested goal is pushed
#[tokio::test]
async fn depth_is_checked_before_a_goal_is_pushed() {
    let mut state = InstanceResolutionState::new(InstanceResolutionLimits::new(1, 4));
    let EnteredInstanceGoal::Active(root) = state.enter_goal(goal(1).await, None).unwrap() else {
        panic!("a new goal should be active");
    };

    assert!(matches!(
        state.enter_goal(goal(2).await, Some(edge(10))),
        Err(InstanceResolutionStateError::Limit {
            limit: InstanceResolutionLimit::Depth { limit: 1 },
            recent_goals,
        }) if recent_goals.len() == 2
    ));

    state.leave_goal(root);
}

// input: resolve a cached goal while another root is active
// premise: the root has one candidate visit remaining
// output: the memo hit leaves that visit available
#[tokio::test]
async fn memo_hits_do_not_consume_candidate_fuel() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let cached_goal = TraitRef::new(symbol(1), Args::new([], &engine));
    let resolved = Ty::new_instance(symbol(10), Args::new([], &engine), &engine);
    let mut state = InstanceResolutionState::new(InstanceResolutionLimits::new(1, 1));
    let EnteredInstanceGoal::Active(active) = state.enter_goal(cached_goal.clone(), None).unwrap()
    else {
        panic!("a new goal should be active");
    };
    state.complete_goal(active, Ok(resolved.clone()));

    let EnteredInstanceGoal::Active(root) = state.enter_goal(goal(2).await, None).unwrap() else {
        panic!("a new root goal should be active");
    };
    assert_eq!(
        state.enter_goal(cached_goal, None),
        Ok(EnteredInstanceGoal::Memoized(Ok(resolved)))
    );
    assert_eq!(state.visit_candidate(symbol(11)), Ok(()));
    state.leave_goal(root);
}

// input: an ambiguous lexical resolution completed for one active goal
// premise: complete non-contextual errors are eligible for memoization
// output: the next entry returns the original structured error
#[tokio::test]
async fn complete_errors_are_memoized_without_losing_details() {
    let required = goal(1).await;
    let candidates = vec![given_id(10, 0), given_id(11, 0)];
    let error =
        InstanceResolutionError::AmbiguousLexical { required: required.clone(), candidates };
    let mut state = InstanceResolutionState::default();
    let EnteredInstanceGoal::Active(active) = state.enter_goal(required.clone(), None).unwrap()
    else {
        panic!("a new goal should be active");
    };

    state.complete_goal(active, Err(error.clone()));

    assert_eq!(state.enter_goal(required, None), Ok(EnteredInstanceGoal::Memoized(Err(error))));
}

// input: one candidate visit under each of two sequential roots
// premise: each root has a budget of one
// output: both candidate visits succeed
#[tokio::test]
async fn each_root_goal_receives_a_fresh_candidate_budget() {
    let mut state = InstanceResolutionState::new(InstanceResolutionLimits::new(1, 1));
    let EnteredInstanceGoal::Active(first) = state.enter_goal(goal(1).await, None).unwrap() else {
        panic!("a new goal should be active");
    };
    state.visit_candidate(symbol(10)).unwrap();
    state.leave_goal(first);

    let EnteredInstanceGoal::Active(second) = state.enter_goal(goal(2).await, None).unwrap() else {
        panic!("a new goal should be active");
    };
    assert_eq!(state.visit_candidate(symbol(11)), Ok(()));
    state.leave_goal(second);
}

// input: Eq[int32] with one lexical dictionary and one global instance
// premise: lexical resolution is the first precedence tier
// output: the lexical PolyVar dictionary
#[tokio::test]
async fn lexical_dictionary_precedes_global_instances() {
    let types = rayc_qbice::create_minimal_engine().await;
    let required = trait_ref(1, [Ty::new_primitive(Primitive::Int32, &types)], &types);
    let global = InstanceCandidate::new(symbol(10), required.clone(), Vec::new());
    let engine =
        engine_with_candidates_and_lexical(&[global], std::slice::from_ref(&required)).await;
    let lexical = given_id(20, 0);
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());

    assert_eq!(
        resolver.resolve_instance(&mut solver, required).await,
        Ok(Ty::new_poly_var(lexical, &engine))
    );
}

// input: Show[int32] resolved by ShowInt given Eq[int32]
// premise: EqInt resolves the recursively introduced requirement
// output: ShowInt[EqInt]
#[tokio::test]
async fn global_instance_constructs_recursive_given_arguments() {
    let types = rayc_qbice::create_minimal_engine().await;
    let int = Ty::new_primitive(Primitive::Int32, &types);
    let eq = trait_ref(1, [int.clone()], &types);
    let show = trait_ref(2, [int], &types);
    let eq_candidate = InstanceCandidate::new(symbol(10), eq.clone(), Vec::new());
    let given = given_id(11, 0);
    let show_candidate =
        InstanceCandidate::new(symbol(11), show.clone(), vec![InstanceCandidateParameter::given(
            given, eq,
        )]);
    let engine = engine_with_candidates(&[show_candidate, eq_candidate]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());
    let eq_term = Ty::new_instance(symbol(10), Args::new([], &engine), &engine);
    let expected = Ty::new_instance(symbol(11), Args::new([eq_term], &engine), &engine);

    assert_eq!(resolver.resolve_instance(&mut solver, show).await, Ok(expected));
}

// input: Eq[int32] with Eq[a] and Eq[int32] candidates
// premise: both candidates are viable and the concrete head is strictly more
// specific output: the concrete instance
#[tokio::test]
async fn unique_most_specific_viable_candidate_wins() {
    let types = rayc_qbice::create_minimal_engine().await;
    let a = given_id(10, 0);
    let int = Ty::new_primitive(Primitive::Int32, &types);
    let required = trait_ref(1, [int.clone()], &types);
    let generic = InstanceCandidate::new(
        symbol(10),
        trait_ref(1, [Ty::new_poly_var(a, &types)], &types),
        vec![InstanceCandidateParameter::ordinary(a, TyKind::Star)],
    );
    let concrete = InstanceCandidate::new(symbol(11), required.clone(), Vec::new());
    let engine = engine_with_candidates(&[generic, concrete]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());
    let expected = Ty::new_instance(symbol(11), Args::new([], &engine), &engine);

    assert_eq!(resolver.resolve_instance(&mut solver, required).await, Ok(expected));
}

// input: Eq[int32] with two identical concrete instance heads
// premise: declaration order is not a semantic tie-breaker
// output: AmbiguousGlobal containing both stable instance IDs
#[tokio::test]
async fn equivalent_viable_heads_are_ambiguous() {
    let types = rayc_qbice::create_minimal_engine().await;
    let required = trait_ref(1, [Ty::new_primitive(Primitive::Int32, &types)], &types);
    let first = InstanceCandidate::new(symbol(11), required.clone(), Vec::new());
    let second = InstanceCandidate::new(symbol(10), required.clone(), Vec::new());
    let engine = engine_with_candidates(&[first, second]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());

    assert_eq!(
        resolver.resolve_instance(&mut solver, required.clone()).await,
        Err(InstanceResolutionError::AmbiguousGlobal {
            required,
            candidates: vec![symbol(10), symbol(11)],
        })
    );
}

// input: Eq[int32] resolved by Loop given Eq[int32]
// premise: the recursive premise re-enters the active canonical goal
// output: Cycle rather than NoInstance
#[tokio::test]
async fn recursive_instance_reports_an_exact_cycle() {
    let types = rayc_qbice::create_minimal_engine().await;
    let required = trait_ref(1, [Ty::new_primitive(Primitive::Int32, &types)], &types);
    let candidate = InstanceCandidate::new(symbol(10), required.clone(), vec![
        InstanceCandidateParameter::given(given_id(10, 0), required.clone()),
    ]);
    let engine = engine_with_candidates(&[candidate]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());

    assert!(matches!(
        resolver.resolve_instance(&mut solver, required).await,
        Err(InstanceResolutionError::Cycle(_))
    ));
}

// input: Eq[int32] with a cyclic candidate followed by a direct candidate
// premise: cycle failure rejects only the candidate branch that introduced it
// output: the direct instance
#[tokio::test]
async fn cycle_failure_does_not_reject_other_candidates() {
    let types = rayc_qbice::create_minimal_engine().await;
    let required = trait_ref(1, [Ty::new_primitive(Primitive::Int32, &types)], &types);
    let cyclic = InstanceCandidate::new(symbol(10), required.clone(), vec![
        InstanceCandidateParameter::given(given_id(10, 0), required.clone()),
    ]);
    let direct = InstanceCandidate::new(symbol(11), required.clone(), Vec::new());
    let engine = engine_with_candidates(&[cyclic, direct]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());
    let expected = Ty::new_instance(symbol(11), Args::new([], &engine), &engine);

    assert_eq!(resolver.resolve_instance(&mut solver, required).await, Ok(expected));
}

// input: resolve Eq[int32] twice through one resolver
// premise: the first successful global search is memoized
// output: one global lookup and the same dictionary term twice
#[tokio::test]
async fn successful_root_resolution_is_memoized_across_calls() {
    let types = rayc_qbice::create_minimal_engine().await;
    let required = trait_ref(1, [Ty::new_primitive(Primitive::Int32, &types)], &types);
    let candidate = InstanceCandidate::new(symbol(10), required.clone(), Vec::new());
    let engine = engine_with_candidates(&[candidate]).await;
    let mut solver = Solver::new(engine.clone());
    let mut resolver = InstanceResolver::new(engine.clone(), site());
    let expected = Ty::new_instance(symbol(10), Args::new([], &engine), &engine);

    assert_eq!(
        resolver.resolve_instance(&mut solver, required.clone()).await,
        Ok(expected.clone())
    );
    assert_eq!(resolver.resolve_instance(&mut solver, required).await, Ok(expected));
}
