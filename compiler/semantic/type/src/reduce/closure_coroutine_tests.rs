use std::{collections::HashMap, sync::Arc};

use qbice::storage::intern::Interned;
use rayc_lexical::tree::{OffsetMode, RelativeLocation, RelativeSpan};
use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor, TrackedEngine};
use rayc_symbol::{
    GlobalSymbolID, SymbolID,
    core_item::{CoreItem, Key as CoreItemKey},
};
use rayc_target::TargetID;

use super::Reduce;
use crate::{
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    subst::Subst,
    ty::{
        Integer, Primitive, Ty,
        application::{Closure, CoroutineView},
        self_instance::SelfInstance,
    },
};

/// The symbols of `trait Def[f]` and its method `call`, which has no
/// parameters of its own.
struct CoreDef {
    trait_id: GlobalSymbolID,
    call_id: GlobalSymbolID,
    f: GlobalPolyVarID,
}

async fn fixture() -> (TrackedEngine, CoreDef) {
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    let def_trait = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let def_call = TargetID::TEST.make_global(SymbolID::from_u128(2));
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

    let mut trait_vars = PolyVarMap::new();
    let f = trait_vars.insert(PolyVar::new_type(engine.intern_unsized("f"), span)).unwrap();

    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (CoreItemKey { role: CoreItem::DefTrait }, def_trait),
        (CoreItemKey { role: CoreItem::DefCall }, def_call),
    ]))));
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([
        (crate::poly_var::Key { symbol_id: def_trait }, engine.intern(trait_vars)),
        (crate::poly_var::Key { symbol_id: def_call }, engine.intern(PolyVarMap::new())),
    ]))));

    let engine = Arc::new(engine).tracked().await;
    (engine, CoreDef {
        trait_id: def_trait,
        call_id: def_call,
        f: GlobalPolyVarID::new(def_trait, f),
    })
}

/// Returns `<closure>(int32) -> int32` with no capture, written in a function
/// without parameters.
fn closure(engine: &TrackedEngine) -> Interned<Ty> {
    let int32 = Ty::new_primitive(Primitive::Integer(Integer::Int32), engine);
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(3));
    Ty::new_closure(
        Closure::new(owner, rayc_arena::ID::new(0), 0),
        [],
        [int32.clone()],
        int32,
        Ty::new_effect_row([], None, engine),
        Ty::new_unit(engine),
        engine,
    )
}

// input: TraitDefCoroutine(Def.call, DefInstance(C))
// premise: C is a closure type
// output: ClosureCoroutine(C)
#[tokio::test]
async fn reduces_closure_call_to_closure_coroutine() {
    let (engine, def) = fixture().await;
    let closure = closure(&engine);
    let instance = Ty::new_def_instance(closure.clone(), &engine);
    let coroutine = Ty::new_trait_def_coroutine(def.call_id, instance, [], &engine);

    assert_eq!(
        Box::pin(coroutine.reduce(&engine, &[])).await.map(|(reduced, _)| reduced),
        Some(Ty::new_closure_coroutine(closure, &engine))
    );
}

// input: CoroutineView of ClosureCoroutine(C) and of
//        TraitDefCoroutine(Def.call, DefInstance(C))
// premise: trait Def[f] declares call, which has no parameters of its own
// output: both run Def.call under {f := C, this := DefInstance(C)}, and
//         instantiate no parameters of their own
#[tokio::test]
async fn closure_coroutine_keeps_signature_of_closure_call() {
    let (engine, def) = fixture().await;
    let closure = closure(&engine);
    let instance = Ty::new_def_instance(closure.clone(), &engine);
    let trait_def_coroutine =
        Ty::new_trait_def_coroutine(def.call_id, instance.clone(), [], &engine);
    let closure_coroutine = Ty::new_closure_coroutine(closure.clone(), &engine);

    let mut expected = Subst::new_empty();
    expected.insert(def.f, closure);
    expected.insert(SelfInstance::new(def.trait_id), instance);

    for coroutine in [&trait_def_coroutine, &closure_coroutine] {
        let view: CoroutineView<'_> = coroutine.as_coroutine_view().unwrap();
        assert_eq!(view.signature_id(&engine).await, def.call_id);
        assert_eq!(view.create_subst(&engine).await, expected);
        assert_eq!(view.instantiation(&engine).await, Subst::new_empty());
    }
}
