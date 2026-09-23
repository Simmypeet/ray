use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
use rayc_semantic_element::{
    drop_plan::{
        DictionaryArgument, DictionaryExpr, DropPlan, get_drop_plan, get_target_drop_plans,
    },
    instance_trait_ref::Key as InstanceTraitRefKey,
    struct_body::{Field, Key as StructBodyKey, StructBody},
};
use rayc_solver::Solver;
use rayc_source_file::GlobalSourceID;
use rayc_symbol::{
    SymbolID,
    core_item::{CoreItem, Key as CoreItemKey},
    member::{Key as MemberKey, Member},
    name::Key as NameKey,
    symbol_kind::{AllInstanceIDs, AllNominalTypeIDs, Key as SymbolKindKey, SymbolKind},
};
use rayc_target::TargetID;
use rayc_type::{
    instance_member::{InstanceMember, Key as InstanceMemberKey},
    poly_var::{GlobalPolyVarID, Key as PolyVarKey, PolyVar, PolyVarID, PolyVarMap},
    trait_ref::TraitRef,
    ty::{
        Mutability, Primitive, Ty,
        application::{Closure, ClosureID},
        args::Args,
    },
    type_definition::Key as TypeDefinitionKey,
    where_clause::{Key as WhereClauseKey, WhereClause},
};

use super::{Evaluator, NominalDropPlanExecutor, TargetDropPlansExecutor};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ROOT_BRANCH_ID };
    RelativeSpan { start: location, end: location, source_id: GlobalSourceID::default() }
}

fn generated(plan: &DropPlan) -> &rayc_semantic_element::drop_plan::GeneratedDropPlan {
    let DropPlan::Generated(plan) = plan else { panic!("expected a generated Drop plan") };
    plan
}

// input: mutually recursive A[t] and B[t], with B owning a value of t
// premise: neither type has an explicit Drop instance
// output: both plans require one Drop[t] dictionary and retain finite field
// recipes
#[tokio::test]
async fn mutually_recursive_plans_share_external_requirement() {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let target = TargetID::TEST;
    let a = target.make_global(SymbolID::from_u128(1));
    let b = target.make_global(SymbolID::from_u128(2));
    let drop_trait = target.make_global(SymbolID::from_u128(3));
    let mut a_params = PolyVarMap::new();
    let a_param = a_params.insert(PolyVar::new_type(engine.intern_unsized("t"), span())).unwrap();
    let mut b_params = PolyVarMap::new();
    let b_param = b_params.insert(PolyVar::new_type(engine.intern_unsized("t"), span())).unwrap();

    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let a_t = tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(a, a_param)));
    let b_t = tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(b, b_param)));
    let b_of_a_t = Ty::new_struct(b, Args::new([a_t.clone()], &tracked), &tracked);
    let a_of_b_t = Ty::new_struct(a, Args::new([b_t.clone()], &tracked), &tracked);
    let mut a_body = StructBody::new();
    let a_next = a_body
        .insert(
            Field::builder().name(tracked.intern_unsized("next")).span(span()).ty(b_of_a_t).build(),
        )
        .unwrap();
    let mut b_body = StructBody::new();
    let b_value = b_body
        .insert(Field::builder().name(tracked.intern_unsized("value")).span(span()).ty(b_t).build())
        .unwrap();
    b_body
        .insert(
            Field::builder().name(tracked.intern_unsized("next")).span(span()).ty(a_of_b_t).build(),
        )
        .unwrap();
    let a_body: Interned<StructBody> = tracked.intern(a_body);
    let b_body: Interned<StructBody> = tracked.intern(b_body);
    let a_params = tracked.intern(a_params);
    let b_params = tracked.intern(b_params);
    drop(tracked);

    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        CoreItemKey { role: CoreItem::DropTrait },
        drop_trait,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        AllNominalTypeIDs { target },
        Arc::<[SymbolID]>::from([a.id, b.id]),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        AllInstanceIDs { target },
        Arc::<[SymbolID]>::from([]),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (PolyVarKey { symbol_id: a }, a_params),
        (PolyVarKey { symbol_id: b }, b_params),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (StructBodyKey { symbol_id: a }, a_body),
        (StructBodyKey { symbol_id: b }, b_body),
    ]))));
    engine_mut.register_executor(Arc::new(TargetDropPlansExecutor));
    engine_mut.register_executor(Arc::new(NominalDropPlanExecutor));

    let tracked = engine.tracked().await;
    let all = tracked.get_target_drop_plans(target).await;
    let a_plan = tracked.get_drop_plan(a).await;
    let b_plan = tracked.get_drop_plan(b).await;
    assert_eq!(all.get(&a.id), Some(&a_plan));
    assert_eq!(all.get(&b.id), Some(&b_plan));
    assert_eq!(generated(&a_plan).requirements(), &[a_t]);
    assert_eq!(generated(&b_plan).requirements(), &[
        tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(b, b_param)))
    ]);
    assert_eq!(generated(&a_plan).fields()[0].field_id(), a_next);
    assert!(matches!(
        generated(&a_plan).fields()[0].dictionary(),
        DictionaryExpr::Generated { external, .. }
            if external == &[DictionaryExpr::External(0)]
    ));
    assert_eq!(generated(&b_plan).fields()[0].field_id(), b_value);
    assert_eq!(generated(&b_plan).fields()[0].dictionary(), &DictionaryExpr::External(0));
}

// input: local Wrapper owns a nominal Leaf from another target
// premise: Leaf has a generated Drop plan in its defining target
// output: Wrapper's plan refers to Leaf while each target map owns only local
// plans
#[tokio::test]
async fn foreign_field_uses_its_defining_targets_plan() {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let local = TargetID::new(10);
    let dependency = TargetID::new(20);
    let wrapper = local.make_global(SymbolID::from_u128(1));
    let leaf = dependency.make_global(SymbolID::from_u128(2));
    let drop_trait = local.make_global(SymbolID::from_u128(3));
    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let leaf_ty = Ty::new_struct(leaf, Args::new([], &tracked), &tracked);
    let mut wrapper_body = StructBody::new();
    wrapper_body
        .insert(
            Field::builder()
                .name(tracked.intern_unsized("leaf"))
                .span(span())
                .ty(leaf_ty.clone())
                .build(),
        )
        .unwrap();
    let wrapper_body = tracked.intern(wrapper_body);
    let leaf_body = tracked.intern(StructBody::new());
    let empty_params = tracked.intern(PolyVarMap::new());
    drop(tracked);

    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        CoreItemKey { role: CoreItem::DropTrait },
        drop_trait,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (AllNominalTypeIDs { target: local }, Arc::<[SymbolID]>::from([wrapper.id])),
        (AllNominalTypeIDs { target: dependency }, Arc::<[SymbolID]>::from([leaf.id])),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (AllInstanceIDs { target: local }, Arc::<[SymbolID]>::from([])),
        (AllInstanceIDs { target: dependency }, Arc::<[SymbolID]>::from([])),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (PolyVarKey { symbol_id: wrapper }, empty_params.clone()),
        (PolyVarKey { symbol_id: leaf }, empty_params),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (StructBodyKey { symbol_id: wrapper }, wrapper_body),
        (StructBodyKey { symbol_id: leaf }, leaf_body),
    ]))));
    engine_mut.register_executor(Arc::new(TargetDropPlansExecutor));
    engine_mut.register_executor(Arc::new(NominalDropPlanExecutor));

    let tracked = engine.tracked().await;
    let plans = tracked.get_target_drop_plans(local).await;
    assert_eq!(plans.len(), 1);
    let wrapper_plan = tracked.get_drop_plan(wrapper).await;
    assert!(matches!(
        generated(&wrapper_plan).fields()[0].dictionary(),
        DictionaryExpr::Generated { nominal, external }
            if nominal == &leaf_ty && external.is_empty()
    ));
    assert!(generated(&*tracked.get_drop_plan(leaf).await).fields().is_empty());
}

// input: Node[t] owns Option[Node[t]], and DropOption[a] requires Drop[a]
// premise: Option's explicit head is simple and Node has no explicit instance
// output: Node passes its generated dictionary, instantiated with Drop[t],
//         to DropOption without a recursive external premise
#[tokio::test]
async fn recursive_field_applies_generated_dictionary_to_explicit_wrapper() {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let target = TargetID::TEST;
    let node = target.make_global(SymbolID::from_u128(1));
    let option = target.make_global(SymbolID::from_u128(2));
    let option_drop = target.make_global(SymbolID::from_u128(3));
    let drop_trait = target.make_global(SymbolID::from_u128(4));
    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;

    let mut node_params = PolyVarMap::new();
    let t = node_params.insert(PolyVar::new_type(tracked.intern_unsized("t"), span())).unwrap();
    let mut option_params = PolyVarMap::new();
    option_params.insert(PolyVar::new_type(tracked.intern_unsized("a"), span())).unwrap();
    let mut instance_params = PolyVarMap::new();
    let instance_a =
        instance_params.insert(PolyVar::new_type(tracked.intern_unsized("a"), span())).unwrap();
    let instance_a_ty = tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(option_drop, instance_a)));
    instance_params
        .insert(PolyVar::new_instance(
            tracked.intern_unsized("aDrop"),
            TraitRef::new(drop_trait, Args::new([instance_a_ty.clone()], &tracked)),
            span(),
        ))
        .unwrap();
    let option_of_a = Ty::new_struct(option, Args::new([instance_a_ty], &tracked), &tracked);
    let explicit_head = TraitRef::new(drop_trait, Args::new([option_of_a], &tracked));
    let node_t = tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(node, t)));
    let node_of_t = Ty::new_struct(node, Args::new([node_t.clone()], &tracked), &tracked);
    let option_of_node = Ty::new_struct(option, Args::new([node_of_t.clone()], &tracked), &tracked);
    let mut node_body = StructBody::new();
    let next = Field::builder()
        .name(tracked.intern_unsized("next"))
        .span(span())
        .ty(option_of_node)
        .build();
    let value = Field::builder()
        .name(tracked.intern_unsized("value"))
        .span(span())
        .ty(node_t.clone())
        .build();
    node_body.insert(next).unwrap();
    node_body.insert(value).unwrap();
    let node_body = tracked.intern(node_body);
    let option_body = tracked.intern(StructBody::new());
    let empty_where = tracked.intern(WhereClause::new(tracked.intern_unsized([])));
    let node_params = tracked.intern(node_params);
    let option_params = tracked.intern(option_params);
    let instance_params = tracked.intern(instance_params);
    drop(tracked);

    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        CoreItemKey { role: CoreItem::DropTrait },
        drop_trait,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        AllNominalTypeIDs { target },
        Arc::<[SymbolID]>::from([node.id, option.id]),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        AllInstanceIDs { target },
        Arc::<[SymbolID]>::from([option_drop.id]),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        InstanceTraitRefKey { symbol_id: option_drop },
        Some(explicit_head),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (PolyVarKey { symbol_id: node }, node_params),
        (PolyVarKey { symbol_id: option }, option_params),
        (PolyVarKey { symbol_id: option_drop }, instance_params),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (StructBodyKey { symbol_id: node }, node_body),
        (StructBodyKey { symbol_id: option }, option_body),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (WhereClauseKey { symbol_id: option }, empty_where.clone()),
        (WhereClauseKey { symbol_id: option_drop }, empty_where),
    ]))));
    engine_mut.register_executor(Arc::new(TargetDropPlansExecutor));

    let tracked = engine.tracked().await;
    let node_plan = tracked.get_target_drop_plans(target).await.get(&node.id).unwrap().clone();
    assert_eq!(generated(&node_plan).requirements(), &[node_t]);
    assert!(matches!(
        generated(&node_plan).fields()[0].dictionary(),
        DictionaryExpr::Explicit { instance_id, arguments }
            if *instance_id == option_drop
                && arguments == &[
                    DictionaryArgument::Type(node_of_t.clone()),
                    DictionaryArgument::Dictionary(Box::new(DictionaryExpr::Generated {
                        nominal: node_of_t,
                        external: vec![DictionaryExpr::External(0)],
                    })),
                ]
    ));
}

// input: an explicit Drop[Option[a]] given Drop[a], applied to
// Option[Impl.Item] premise: Impl.Item definitionally reduces to int32 after
// head substitution output: the given is a no-op Drop[int32], with no external
// requirement
#[tokio::test]
async fn concrete_associated_argument_does_not_become_external_requirement() {
    let engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let target = TargetID::TEST;
    let owner = target.make_global(SymbolID::from_u128(1));
    let option = target.make_global(SymbolID::from_u128(2));
    let option_drop = target.make_global(SymbolID::from_u128(3));
    let drop_trait = target.make_global(SymbolID::from_u128(4));
    let trait_member = target.make_global(SymbolID::from_u128(5));
    let implementation = target.make_global(SymbolID::from_u128(6));
    let implementation_member = target.make_global(SymbolID::from_u128(7));

    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let int_ty = Ty::new_primitive(Primitive::Int32, &tracked);
    let instance = Ty::new_instance(implementation, Args::new([], &tracked), &tracked);
    let projection = Ty::new_instance_associated(trait_member, instance, [], &tracked);
    let option_of_projection = Ty::new_struct(option, Args::new([projection], &tracked), &tracked);

    let mut params = PolyVarMap::new();
    let a = params.insert(PolyVar::new_type(tracked.intern_unsized("a"), span())).unwrap();
    let a_ty = tracked.intern(Ty::PolyVar(GlobalPolyVarID::new(option_drop, a)));
    params
        .insert(PolyVar::new_instance(
            tracked.intern_unsized("aDrop"),
            TraitRef::new(drop_trait, Args::new([a_ty.clone()], &tracked)),
            span(),
        ))
        .unwrap();
    let head = TraitRef::new(
        drop_trait,
        Args::new([Ty::new_struct(option, Args::new([a_ty], &tracked), &tracked)], &tracked),
    );
    let params = tracked.intern(params);
    let empty_params = tracked.intern(PolyVarMap::new());
    let item_name: Interned<str> = tracked.intern_unsized("Item");
    let mut members = Member::default();
    let _ = members.insert(item_name.clone(), implementation_member.id);
    let members = tracked.intern(members);
    let correspondence = tracked.intern(InstanceMember::new(
        trait_member,
        implementation_member,
        rayc_type::subst::Subst::new_empty(),
    ));
    drop(tracked);

    let engine_mut = Arc::get_mut(&mut engine).unwrap();
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        InstanceTraitRefKey { symbol_id: option_drop },
        Some(head),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (PolyVarKey { symbol_id: option_drop }, params),
        (PolyVarKey { symbol_id: implementation }, empty_params.clone()),
        (PolyVarKey { symbol_id: trait_member }, empty_params),
    ]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        NameKey { symbol_id: trait_member },
        item_name,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        MemberKey { symbol_id: implementation },
        members,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        SymbolKindKey { symbol_id: implementation_member },
        SymbolKind::InstanceType,
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        InstanceMemberKey { symbol_id: implementation_member },
        Some(correspondence),
    )]))));
    engine_mut.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        TypeDefinitionKey { symbol_id: implementation_member },
        int_ty.clone(),
    )]))));

    let tracked = engine.tracked().await;
    let plans =
        FxHashMap::from_iter([(option.id, tracked.intern(DropPlan::Explicit(option_drop)))]);
    let mut evaluator = Evaluator {
        engine: &tracked,
        solver: Solver::with_givens(tracked.clone(), owner, []),
        current_nominal: owner,
        plans: &plans,
        requirements: Vec::new(),
    };
    assert_eq!(evaluator.resolve(option_of_projection).await.unwrap(), DictionaryExpr::Explicit {
        instance_id: option_drop,
        arguments: vec![
            DictionaryArgument::Type(int_ty.clone()),
            DictionaryArgument::Dictionary(Box::new(DictionaryExpr::NoOp(int_ty))),
        ],
    });
    assert!(evaluator.requirements.is_empty());
}

// input: a field of closure type capturing `t` by value and an `int32` by
//        reference
// premise: the struct has no explicit Drop instance
// output: one dictionary per capture: an external Drop[t] requirement for the
//         by-value capture and a no-op for the borrowed pointer
#[tokio::test]
async fn closure_field_resolves_one_dictionary_per_capture() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let t = engine.intern(Ty::PolyVar(GlobalPolyVarID::new(owner, PolyVarID::new(0))));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let borrowed = Ty::new_pointer(int_ty.clone(), Mutability::Immutable, &engine);

    let captures = Ty::new_tuple(engine.intern_unsized([t.clone(), borrowed.clone()]), &engine);
    let closure = Ty::new_closure(
        Closure::new(owner, ClosureID::new(0), 1),
        [t.clone()],
        [],
        Ty::new_unit(&engine),
        Ty::new_effect_row([], None, &engine),
        captures,
        &engine,
    );

    let plans = FxHashMap::default();
    let mut evaluator = Evaluator {
        engine: &engine,
        solver: Solver::with_givens(engine.clone(), owner, []),
        current_nominal: owner,
        plans: &plans,
        requirements: Vec::new(),
    };

    assert_eq!(evaluator.resolve(closure.clone()).await.unwrap(), DictionaryExpr::Closure {
        closure,
        captures: vec![DictionaryExpr::External(0), DictionaryExpr::NoOp(borrowed)],
    });
    assert_eq!(evaluator.requirements, vec![t]);
}
