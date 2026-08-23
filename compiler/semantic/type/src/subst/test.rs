use rayc_arena::ID;
use rayc_symbol::{GlobalSymbolID, MemberID};

use super::*;
use crate::{
    poly_var::PolyVar,
    ty::{Mutability, Primitive, TyKind},
};

// input: {T0 -> T1} composed with {T1 -> int32}
// premise: {}
// output: {T0 -> int32, T1 -> int32}
#[tokio::test]
async fn compose_rewrites_existing_bindings() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let t0 = TyInference::new(TyKind::Star, 0);
    let t1 = TyInference::new(TyKind::Star, 1);
    let t1_ty = engine.intern(Ty::Inference(t1));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let mut subst = Subst::new_singleton(t0, t1_ty);
    let other = Subst::new_singleton(t1, int32.clone());

    subst.compose(&other, &engine);

    assert_eq!(subst.0.len(), 2);
    assert_eq!(subst.get(&t0), Some(&int32));
    assert_eq!(subst.get(&t1), Some(&int32));
}

// input: {T0 -> T1} composed with {T0 -> int32}
// premise: both substitutions bind T0
// output: {T0 -> T1}
#[tokio::test]
async fn compose_preserves_first_substitution_domain_precedence() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let t0 = TyInference::new(TyKind::Star, 0);
    let t1 = TyInference::new(TyKind::Star, 1);
    let t1_ty = engine.intern(Ty::Inference(t1));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let mut subst = Subst::new_singleton(t0, t1_ty.clone());
    let other = Subst::new_singleton(t0, int32);

    subst.compose(&other, &engine);

    assert_eq!(subst.0.len(), 1);
    assert_eq!(subst.get(&t0), Some(&t1_ty));
}

// input: {T0 -> *mut T1} composed with {T1 -> int32}
// premise: {}
// output: {T0 -> *mut int32, T1 -> int32}
#[tokio::test]
async fn compose_rewrites_nested_types() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let t0 = TyInference::new(TyKind::Star, 0);
    let t1 = TyInference::new(TyKind::Star, 1);
    let t1_ty = engine.intern(Ty::Inference(t1));
    let pointer_to_t1 = Ty::new_pointer(t1_ty, Mutability::Mutable, &engine);
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let pointer_to_int32 = Ty::new_pointer(int32.clone(), Mutability::Mutable, &engine);
    let mut subst = Subst::new_singleton(t0, pointer_to_t1);
    let other = Subst::new_singleton(t1, int32.clone());

    subst.compose(&other, &engine);

    assert_eq!(subst.0.len(), 2);
    assert_eq!(subst.get(&t0), Some(&pointer_to_int32));
    assert_eq!(subst.get(&t1), Some(&int32));
}

// input: `{T0 -> int32, P0 -> bool}` applied to `T0` and `P0`
// premise: `T0` is inference; `P0` is global polymorphic
// output: both variables are replaced by their corresponding primitive types
#[tokio::test]
async fn substitutes_inference_and_polymorphic_variables() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let inference = TyInference::new(TyKind::Star, 0);
    let poly: GlobalPolyVarID = MemberID::new(GlobalSymbolID::default(), ID::<PolyVar>::new(0));
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let inference_ty = engine.intern(Ty::Inference(inference));
    let poly_ty = Ty::new_poly_var(poly, &engine);
    let subst: Subst =
        [(Var::Inference(inference), int32.clone()), (Var::Poly(poly), bool_ty.clone())]
            .into_iter()
            .collect();

    assert_eq!(subst.get(&Var::Inference(inference)), Some(&int32));
    assert_eq!(subst.get(&Var::Poly(poly)), Some(&bool_ty));
    assert_eq!(inference_ty.apply_subst(&subst, &engine), Some(int32));
    assert_eq!(poly_ty.apply_subst(&subst, &engine), Some(bool_ty));
}
