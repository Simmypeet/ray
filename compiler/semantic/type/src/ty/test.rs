use rayc_arena::ID;
use rayc_symbol::{GlobalSymbolID, MemberID};

use super::{Mutability, Primitive, Ty, TyApplicationView};
use crate::poly_var::{GlobalPolyVarID, PolyVar};

// input: immutable and mutable pointers to int32
// premise: {}
// output: `*int32` and `*mut int32`, with matching pointer views
#[tokio::test]
async fn pointer_mutability_is_formatted_and_exposed() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let immutable = Ty::new_pointer(int32.clone(), Mutability::Immutable, &engine);
    let mutable = Ty::new_pointer(int32, Mutability::Mutable, &engine);

    assert_eq!(immutable.to_string(), "*int32");
    assert_eq!(mutable.to_string(), "*mut int32");

    let Ty::Application(application) = &*mutable else {
        panic!("a pointer should be a type application");
    };
    let TyApplicationView::Pointer(pointer) = application.view() else {
        panic!("expected a pointer view");
    };
    assert_eq!(pointer.mutability(), Mutability::Mutable);
}

// input: function-qualified polymorphic variable ID 0
// premise: the ID identifies a rigid variable declared by its parent function
// output: `Ty::PolyVar` retains that qualified identity
#[tokio::test]
async fn polymorphic_variable_retains_qualified_identity() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let id: GlobalPolyVarID = MemberID::new(GlobalSymbolID::default(), ID::<PolyVar>::new(0));
    let ty = Ty::new_poly_var(id, &engine);

    assert_eq!(&*ty, &Ty::PolyVar(id));
    assert_eq!(ty.to_string(), "{poly#0}");
}
