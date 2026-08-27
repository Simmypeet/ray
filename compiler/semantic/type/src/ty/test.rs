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
