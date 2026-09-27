use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::{SymbolID, member::Member, symbol_kind::SymbolKind};
use rayc_target::TargetID;

use super::Reduce;
use crate::{
    instance_member::InstanceMember,
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    subst::Subst,
    ty::{Primitive, Ty, TyKind, args::Args, inference::Inference},
};

async fn fixture(local_argument: bool) -> (TrackedEngine, Interned<Ty>, Interned<Ty>) {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let trait_member = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let instance_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let member_id = TargetID::TEST.make_global(SymbolID::from_u128(3));
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
    let mut instance_vars = PolyVarMap::new();
    let a = instance_vars.insert(PolyVar::new_type(engine.intern_unsized("a"), span)).unwrap();
    let mut trait_vars = PolyVarMap::new();
    let mut member_vars = PolyVarMap::new();
    let local_vars = local_argument.then(|| {
        let x = trait_vars.insert(PolyVar::new_type(engine.intern_unsized("x"), span)).unwrap();
        let b = member_vars.insert(PolyVar::new_type(engine.intern_unsized("b"), span)).unwrap();
        (GlobalPolyVarID::new(trait_member, x), GlobalPolyVarID::new(member_id, b))
    });
    let mut members = Member::default();
    let _ = members.insert(engine.intern_unsized("Inner"), member_id.id);
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        rayc_symbol::name::Key { symbol_id: trait_member },
        engine.intern_unsized("Inner"),
    )]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        rayc_symbol::member::Key { symbol_id: instance_id },
        engine.intern(members),
    )]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        rayc_symbol::symbol_kind::Key { symbol_id: member_id },
        SymbolKind::InstanceType,
    )]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (crate::poly_var::Key { symbol_id: instance_id }, engine.intern(instance_vars)),
        (crate::poly_var::Key { symbol_id: trait_member }, engine.intern(trait_vars)),
    ]))));

    // Model the checked mapping from trait-local x to implementation-local b.
    let a = engine.intern(Ty::PolyVar(GlobalPolyVarID::new(instance_id, a)));
    let mut mapping = Subst::new_empty();
    let second = local_vars.map_or_else(
        || a.clone(),
        |(x, b)| {
            let b = engine.intern(Ty::PolyVar(b));
            mapping.insert(x, b.clone());
            b
        },
    );
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        crate::instance_member::Key { symbol_id: member_id },
        Some(engine.intern(InstanceMember::new(trait_member, member_id, mapping))),
    )]))));

    let definition_args = engine.intern_unsized([a, second]);
    let mut engine = Arc::new(engine);
    let tracked = engine.clone().tracked().await;
    let definition = Ty::new_tuple(definition_args, &tracked);
    drop(tracked);
    Arc::get_mut(&mut engine).unwrap().register_executor(Arc::new(PrecomputedExecutor::new(
        HashMap::from([(crate::type_definition::Key { symbol_id: member_id }, definition)]),
    )));
    let engine = engine.tracked().await;
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let instance = Ty::new_instance(instance_id, Args::new([int_ty.clone()], &engine), &engine);
    let projection = Ty::new_instance_associated(
        trait_member,
        instance,
        local_argument.then_some(bool_ty.clone()),
        &engine,
    );
    let expected = Ty::new_tuple(
        engine.intern_unsized([int_ty.clone(), if local_argument { bool_ty } else { int_ty }]),
        &engine,
    );
    (engine, projection, expected)
}

// input: Test[int32].Inner
// premise: Test[a].Inner = (a, a)
// output: (int32, int32)
#[tokio::test]
async fn substitutes_enclosing_instance_arguments() {
    let (engine, projection, expected) = fixture(false).await;
    assert_eq!(
        projection
            .reduce(&engine, &[], &mut crate::constraint::outlives::OutlivesSink::dropping())
            .await,
        Some(expected)
    );
}

// input: Test[int32].Inner[bool]
// premise: trait Inner[x] maps x to b in Test[a].Inner[b] = (a, b)
// output: (int32, bool)
#[tokio::test]
async fn substitutes_checked_member_mapping_and_instance_arguments() {
    let (engine, projection, expected) = fixture(true).await;
    assert_eq!(
        projection
            .reduce(&engine, &[], &mut crate::constraint::outlives::OutlivesSink::dropping())
            .await,
        Some(expected)
    );
}

// input: (Test[int32].Inner,)
// premise: Test[a].Inner = (a, a)
// output: ((int32, int32),)
#[tokio::test]
async fn reduces_associated_types_in_descendants() {
    let (engine, projection, expected) = fixture(false).await;
    let tuple = Ty::new_tuple(engine.intern_unsized([projection]), &engine);
    let expected = Ty::new_tuple(engine.intern_unsized([expected]), &engine);
    assert_eq!(
        tuple
            .reduce(&engine, &[], &mut crate::constraint::outlives::OutlivesSink::dropping())
            .await,
        Some(expected)
    );
}

// input: ?instance.Inner[int32]
// premise: the instance is not yet known
// output: no reduction
#[tokio::test]
async fn leaves_unknown_instances_unreduced() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let projection = Ty::new_instance_associated(
        TargetID::TEST.make_global(SymbolID::from_u128(1)),
        engine.intern(Ty::Inference(Inference::new(TyKind::Instance, 0))),
        [Ty::new_primitive(Primitive::Int32, &engine)],
        &engine,
    );
    assert_eq!(
        projection
            .reduce(&engine, &[], &mut crate::constraint::outlives::OutlivesSink::dropping())
            .await,
        None
    );
}
