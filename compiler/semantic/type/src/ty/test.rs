use super::{Mutability, Primitive, Ty};

// input: def(*int32, (bool, float32)) -> int32 \ {}
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
    let root = Ty::new_lambda(
        [pointer_ty.clone(), tuple_ty.clone()],
        int_ty.clone(),
        effect_row.clone(),
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
// premise: ?instance maps to I and ?a maps to int32
// output: I.Inner[int32, bool], with Star kind and no remaining inference
// variables
#[tokio::test]
async fn associated_type_substitution_replaces_instance_and_member_arguments() {
    use rayc_symbol::SymbolID;
    use rayc_target::TargetID;

    use super::{
        TyKind,
        application::{Application, Constant, View},
        args::Args,
        inference::Inference,
    };
    use crate::subst::{Subst, Substitutable};

    let engine = rayc_qbice::create_minimal_engine().await;
    let member_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
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
    assert_eq!(original.reduce(&engine, &[]).await, None);
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
