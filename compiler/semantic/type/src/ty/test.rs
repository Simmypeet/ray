use std::sync::Arc;

use qbice::{serialize::Plugin, stable_hash::SeededStableHasherBuilder, storage::intern::Interned};
use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_symbol::{GlobalSymbolID, MemberID};
use rayc_target::TargetID;

use super::{Mutability, Primitive, Ty, TyApplicationView};
use crate::{
    poly_var::{GlobalPolyVarID, PolyVar, PolyVarMap},
    ty::TyKind,
};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(
        location,
        location,
        TargetID::TEST.make_global(rayc_source_file::LocalSourceID::new(0, 0)),
    )
}

async fn engine_with_poly_vars(
    symbol_id: GlobalSymbolID,
    poly_vars: PolyVarMap,
) -> rayc_qbice::TrackedEngine {
    let engine = Arc::new(
        rayc_qbice::Engine::new_with(
            Plugin::default(),
            rayc_qbice::InMemoryFactory,
            SeededStableHasherBuilder::new(0),
        )
        .await
        .unwrap(),
    );
    let mut input_session = engine.input_session().await;
    input_session.set_input(crate::poly_var::Key { symbol_id }, engine.intern(poly_vars)).await;
    input_session.commit().await;
    engine.tracked().await
}

// input: immutable and mutable pointers to int32
// premise: {}
// output: `*int32` and `*mut int32`, with matching pointer views
#[tokio::test]
async fn pointer_mutability_is_formatted_and_exposed() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let immutable = Ty::new_pointer(int32.clone(), Mutability::Immutable, &engine);
    let mutable = Ty::new_pointer(int32, Mutability::Mutable, &engine);

    assert_eq!(immutable.display(&engine).await.to_string(), "*int32");
    assert_eq!(mutable.display(&engine).await.to_string(), "*mut int32");

    let Ty::Application(application) = &*mutable else {
        panic!("a pointer should be a type application");
    };
    let TyApplicationView::Pointer(pointer) = application.view() else {
        panic!("expected a pointer view");
    };
    assert_eq!(pointer.mutability(), Mutability::Mutable);
}

// input: `Ty::PolyVar` for global variable `{a}`
// premise: the tracked engine contains `a` in its parent `PolyVarMap`
// output: the display wrapper formats the variable as `{a}`
#[tokio::test]
async fn polymorphic_variable_retains_qualified_identity() {
    let symbol_id = GlobalSymbolID::default();
    let mut poly_vars = PolyVarMap::new();
    let poly_var_id = poly_vars.insert(PolyVar::new(
        Interned::new_duplicating_unsized("a"),
        TyKind::Star,
        span(),
    ));
    let engine = engine_with_poly_vars(symbol_id, poly_vars).await;
    let id: GlobalPolyVarID = MemberID::new(symbol_id, poly_var_id);
    let ty = Ty::new_poly_var(id, &engine);

    assert_eq!(&*ty, &Ty::PolyVar(id));
    assert_eq!(ty.display(&engine).await.to_string(), "{a}");
}
