use super::*;
use crate::ty::{Mutability, Primitive};

// input: *int32 <: *mut int32
// premise: pointer types are invariant in mutability
// output: Conflicted
#[tokio::test]
async fn immutable_pointer_is_not_a_mutable_pointer() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let immutable = Ty::new_pointer(int32.clone(), Mutability::Immutable, &engine);
    let mutable = Ty::new_pointer(int32, Mutability::Mutable, &engine);
    let mut solver = Solver::new(engine);

    let result = solver.entail_subtype(&Subtype::new(immutable, mutable));

    assert_eq!(result, Err(Error::Conflicted));
}

// input: *mut int32 <: *int32
// premise: pointer types are invariant in mutability
// output: Conflicted
#[tokio::test]
async fn mutable_pointer_is_not_an_immutable_pointer() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let immutable = Ty::new_pointer(int32.clone(), Mutability::Immutable, &engine);
    let mutable = Ty::new_pointer(int32, Mutability::Mutable, &engine);
    let mut solver = Solver::new(engine);

    let result = solver.entail_subtype(&Subtype::new(mutable, immutable));

    assert_eq!(result, Err(Error::Conflicted));
}
