use super::{Mutability, Primitive, Ty};

// input: Box[a], substituted with a := int32, and Other[int32]
// premise: struct applications are nominal and carry substitutable type
// arguments output: Box[int32] has star kind and does not match Other[int32]
#[tokio::test]
async fn struct_application_preserves_nominal_identity_and_substitutes_arguments() {
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::{TyKind, application::View, args::Args};
    use crate::{
        poly_var::{GlobalPolyVarID, PolyVarID},
        subst::{Subst, Substitutable},
    };

    let engine = rayc_qbice::create_minimal_engine().await;
    let struct_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let other_struct_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let poly_var = GlobalPolyVarID::new(struct_id, PolyVarID::new(0));
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let generic = Ty::new_struct(
        struct_id,
        Args::new([Ty::new_poly_var(poly_var, &engine)], &engine),
        &engine,
    );

    let instantiated =
        generic.apply_subst_or_clone(&Subst::new_singleton(poly_var, int_ty.clone()), &engine);
    let other = Ty::new_struct(other_struct_id, Args::new([int_ty.clone()], &engine), &engine);

    let View::Struct(struct_) = instantiated.unwrap_as_application_view() else {
        panic!("expected a struct application");
    };
    assert_eq!(struct_.symbol_id(), struct_id);
    assert_eq!(struct_.args(), std::slice::from_ref(&int_ty));
    assert_eq!(instantiated.kind_of(&engine).await, TyKind::Star);
    assert!(!instantiated.has_same_type_constructor(&other));
}

// input: closure(owner, 0)[b, a](int32) -> int32 with empty effect and captures
// premise: owner.b maps to bool and its parent.a maps to float32
// output: owner arguments become [bool, float32]; identity and signature are
// preserved
#[tokio::test]
async fn closure_substitution_preserves_unused_owner_arguments_separately_from_signature() {
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::application::{Closure, ClosureID, View};
    use crate::{
        poly_var::{GlobalPolyVarID, PolyVarID},
        subst::{Subst, Substitutable},
    };

    let engine = rayc_qbice::create_minimal_engine().await;
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let parent = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let b = GlobalPolyVarID::new(owner, PolyVarID::new(0));
    let a = GlobalPolyVarID::new(parent, PolyVarID::new(0));
    let closure_id = ClosureID::new(0);
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let float_ty = Ty::new_primitive(Primitive::Float32, &engine);
    let effect = Ty::new_effect_row([], None, &engine);
    let captures = Ty::new_unit(&engine);
    let ty = Ty::new_closure(
        Closure::new(owner, closure_id, 2),
        [engine.intern(Ty::PolyVar(b)), engine.intern(Ty::PolyVar(a))],
        [int_ty.clone()],
        int_ty.clone(),
        effect.clone(),
        captures.clone(),
        &engine,
    );
    let mut subst = Subst::new_singleton(b, bool_ty.clone());
    subst.insert(a, float_ty.clone());

    let instantiated = ty.apply_subst_or_clone(&subst, &engine);

    let View::Closure(view) = instantiated.unwrap_as_application_view() else {
        panic!("expected a closure");
    };
    assert_eq!(view.owner_id(), owner);
    assert_eq!(view.local_closure_id(), closure_id);
    assert_eq!(view.owner_arguments(), [bool_ty, float_ty]);
    assert_eq!(view.params(), std::slice::from_ref(&int_ty));
    assert_eq!(view.return_type(), &int_ty);
    assert_eq!(view.effect_row(), &effect);
    assert_eq!(view.captured_tuple(), &captures);
}

// input: three captureless closures with identical signatures
// premise: the first two have different owners; the third has another local ID
// output: none of their nominal types structurally match
#[tokio::test]
async fn closure_identity_distinguishes_owners_and_local_closures() {
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::application::{Closure, ClosureID};

    let engine = rayc_qbice::create_minimal_engine().await;
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let other_owner = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let closures = [(owner, 0), (other_owner, 0), (owner, 1)].map(|(owner, local_id)| {
        Ty::new_closure(
            Closure::new(owner, ClosureID::new(local_id), 0),
            [],
            [],
            Ty::new_unit(&engine),
            Ty::new_effect_row([], None, &engine),
            Ty::new_unit(&engine),
            &engine,
        )
    });

    for (index, left) in closures.iter().enumerate() {
        for right in &closures[index + 1..] {
            let Ty::Application(left) = left.as_ref() else { panic!("expected application") };
            let Ty::Application(right) = right.as_ref() else { panic!("expected application") };
            assert!(left.structural_match(right).is_none());
        }
    }
}

// input: (*int32, (bool, float32), int32, {})
// premise: nested applications are traversed in argument order
// output: root, *int32, (bool, float32), int32, {}, int32, bool, float32
#[tokio::test]
async fn recursive_iter_yields_root_and_descendants_in_breadth_first_order() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let float_ty = Ty::new_primitive(Primitive::Float32, &engine);
    let pointer_ty = Ty::new_pointer(int_ty.clone(), Mutability::Immutable, &engine);
    let tuple_ty =
        Ty::new_tuple(engine.intern_unsized([bool_ty.clone(), float_ty.clone()]), &engine);
    let effect_row = Ty::new_effect_row([], None, &engine);
    let root = Ty::new_tuple(
        engine.intern_unsized([
            pointer_ty.clone(),
            tuple_ty.clone(),
            int_ty.clone(),
            effect_row.clone(),
        ]),
        &engine,
    );

    let recursive_types = Ty::interned_recursive_iter(&root).collect::<Vec<_>>();

    assert_eq!(recursive_types, vec![
        &root,
        &pointer_ty,
        &tuple_ty,
        &int_ty,
        &effect_row,
        &int_ty,
        &bool_ty,
        &float_ty
    ]);
}

// input: ?instance.Inner[?a, bool]
// premise: Inner has Star kind; ?instance maps to I and ?a maps to int32
// output: I.Inner[int32, bool], with Star kind and no remaining inference
// variables
#[tokio::test]
async fn associated_type_substitution_replaces_instance_and_member_arguments() {
    use std::{collections::HashMap, sync::Arc};

    use rayc_qbice::{Engine, InMemoryFactory, PrecomputedExecutor};
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::{
        TyKind,
        application::{Application, Constant, View},
        args::Args,
        inference::Inference,
    };
    use crate::subst::{Subst, Substitutable};

    let member_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    // Supply the declaration kind queried by the substituted projection.
    let mut engine = Engine::new_with(
        qbice::serialize::Plugin::default(),
        InMemoryFactory,
        qbice::stable_hash::SeededStableHasherBuilder::new(0),
    )
    .await
    .unwrap();
    engine.register_executor(Arc::new(PrecomputedExecutor::new(HashMap::from([(
        crate::associated_type_kind::Key { symbol_id: member_id },
        TyKind::Star,
    )]))));
    let engine = Arc::new(engine).tracked().await;
    let instance_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let instance_var = Inference::new(TyKind::Instance, 0);
    let arg_var = Inference::new(TyKind::Star, 1);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let int_ty = Ty::new_primitive(Primitive::Int32, &engine);
    let projection = Ty::new_instance_associated(
        member_id,
        engine.intern(Ty::Inference(instance_var)),
        [engine.intern(Ty::Inference(arg_var)), bool_ty.clone()],
        &engine,
    );
    let instance = Ty::new_instance(instance_id, Args::new([], &engine), &engine);
    let mut substitution = Subst::new_singleton(instance_var, instance.clone());
    substitution.insert(arg_var, int_ty.clone());

    let result = projection.apply_subst_or_clone(&substitution, &engine);

    let expected = engine.intern(Ty::Application(Application::new(
        Constant::InstanceAssociated(member_id),
        engine.intern_unsized([instance.clone(), int_ty.clone(), bool_ty.clone()]),
    )));
    assert_eq!(result, expected);
    assert_eq!(result.kind_of(&engine).await, TyKind::Star);
    assert!(!result.contains_inference());
    let Ty::Application(application) = result.as_ref() else { panic!("expected application") };
    let View::InstanceAssociated(associated) = application.view() else {
        panic!("expected associated type")
    };
    assert_eq!(associated.symbol_id(), member_id);
    assert_eq!(associated.instance(), &instance);
    assert_eq!(associated.args(), &[int_ty, bool_ty]);
}

// input: (this.Item, other.Item), followed by this := ?i and ?i := I
// premise: self binders are scoped by trait and substitute inside projections
// output: (I.Item, other.Item), with the unrelated projection still abstract
#[tokio::test]
async fn self_instance_substitution_composes_without_capturing_other_traits() {
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use crate::{
        reduce::Reduce,
        subst::{Subst, Substitutable},
        ty::{TyKind, args::Args, inference::Inference, self_instance::SelfInstance},
    };

    let engine = rayc_qbice::create_minimal_engine().await;
    let id = |n| TargetID::TEST.make_global(SymbolID::from_u128(n));
    let this = SelfInstance::new(id(1));
    let projection = |binder| {
        Ty::new_instance_associated(id(3), engine.intern(Ty::SelfInstance(binder)), [], &engine)
    };
    let original = projection(this);
    assert_eq!(
        original
            .reduce(&engine, &[], &mut crate::constraint::outlives::OutlivesSink::dropping())
            .await,
        None
    );
    let other = projection(SelfInstance::new(id(2)));
    let tuple = Ty::new_tuple(engine.intern_unsized([original, other.clone()]), &engine);
    let inference = Inference::new(TyKind::Instance, 0);
    let mut subst = Subst::new_singleton(this, engine.intern(Ty::Inference(inference)));
    let instance = Ty::new_instance(id(4), Args::new([], &engine), &engine);
    subst.compose(&Subst::new_singleton(inference, instance.clone()), &engine);
    let expected = Ty::new_tuple(
        engine.intern_unsized([Ty::new_instance_associated(id(3), instance, [], &engine), other]),
        &engine,
    );
    assert_eq!(tuple.apply_subst_or_clone(&subst, &engine), expected);
}
