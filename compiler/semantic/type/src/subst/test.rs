use super::*;
use crate::ty::{Mutability, Primitive, TyKind};

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
