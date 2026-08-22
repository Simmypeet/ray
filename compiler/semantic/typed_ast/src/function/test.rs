use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_source_file::LocalSourceID;
use rayc_target::TargetID;
use rayc_type::{
    subst::{MutSubstitutable, Subst},
    ty::{Mutability, Primitive, Ty, TyInference, TyKind},
};

use super::Function;
use crate::{
    name_binding::{NameBinding, Source},
    typed_expr::{TypedExpr, TypedExprKind, literal::Literal},
    variable::Variable,
};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
}

#[tokio::test]
async fn substitution_rewrites_all_types_in_a_function() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let inference = TyInference::new(TyKind::Star, 0);
    let inference_ty = engine.intern(Ty::Inference(inference));
    let pointer_to_inference =
        Ty::new_pointer(inference_ty.clone(), Mutability::Immutable, &engine);
    let int32 = Ty::new_primitive(Primitive::Int32, &engine);
    let pointer_to_int32 = Ty::new_pointer(int32.clone(), Mutability::Immutable, &engine);
    let subst = Subst::new_singleton(inference, int32.clone());
    let mut function = Function::default();

    let variable_id = function.insert_variable(Variable::new(inference_ty, span()));
    let name_binding_id = function.insert_name_binding(
        NameBinding::builder()
            .ty(pointer_to_inference.clone())
            .name(engine.intern_unsized("value"))
            .source(Source::Variable(variable_id))
            .mutable(false)
            .span(span())
            .build(),
    );
    let expression_id = function.insert_expression(TypedExpr::new(
        TypedExprKind::Literal(Literal::Numeric(0)),
        span(),
        pointer_to_inference,
    ));

    function.apply_mut_subst(&subst, &engine);

    assert_eq!(function.get_variable(variable_id).ty(), &int32);
    assert_eq!(function.get_variable(variable_id).span(), span());
    assert_eq!(function.get_name_binding(name_binding_id).ty(), &pointer_to_int32);
    assert_eq!(function.get_expression(expression_id).ty(), &pointer_to_int32);
}
