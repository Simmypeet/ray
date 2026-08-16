use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_source_file::LocalSourceID;
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::ty::{Primitive, Ty};
use rayc_typed_ast::typed_expr::{
    TypedExpr, TypedExprID, TypedExprKind, binary::BinaryOp, literal::Literal,
};

use crate::{diagnostic::Diagnostic, tast_builder::TAstBuilder};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
}

fn literal(builder: &mut TAstBuilder, value: u128) -> TypedExprID {
    let ty = Ty::new_primitive(Primitive::Int32, builder.engine());
    builder.insert_expression(TypedExpr::new(
        TypedExprKind::Literal(Literal::Numeric(value)),
        span(),
        ty,
    ))
}

#[tokio::test]
async fn assignment_is_right_associative() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let function_id = TargetID::TEST.make_global(SymbolID::default());
    let mut builder = TAstBuilder::new(engine, function_id);
    let a = literal(&mut builder, 1);
    let b = literal(&mut builder, 2);
    let c = literal(&mut builder, 3);

    let root = builder.reduce_with_precedence(a, [(BinaryOp::Assign, b), (BinaryOp::Assign, c)]);
    let (function, _) = builder.finish();

    let TypedExprKind::Binary(outer) = function.get_expression(root).kind() else {
        panic!("expected an assignment");
    };
    assert_eq!(outer.left(), a);
    assert_eq!(outer.operator(), BinaryOp::Assign);

    let TypedExprKind::Binary(inner) = function.get_expression(outer.right()).kind() else {
        panic!("expected a nested assignment");
    };
    assert_eq!(inner.left(), b);
    assert_eq!(inner.operator(), BinaryOp::Assign);
    assert_eq!(inner.right(), c);
}

#[tokio::test]
async fn assignment_has_lower_precedence_than_addition() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let function_id = TargetID::TEST.make_global(SymbolID::default());
    let mut builder = TAstBuilder::new(engine, function_id);
    let a = literal(&mut builder, 1);
    let b = literal(&mut builder, 2);
    let c = literal(&mut builder, 3);

    let root = builder.reduce_with_precedence(a, [(BinaryOp::Assign, b), (BinaryOp::Plus, c)]);
    let (function, _) = builder.finish();

    let TypedExprKind::Binary(assignment) = function.get_expression(root).kind() else {
        panic!("expected an assignment");
    };
    assert_eq!(assignment.operator(), BinaryOp::Assign);
    let TypedExprKind::Binary(addition) = function.get_expression(assignment.right()).kind() else {
        panic!("expected addition on the right-hand side");
    };
    assert_eq!(addition.operator(), BinaryOp::Plus);
    assert_eq!(addition.left(), b);
    assert_eq!(addition.right(), c);
}

#[tokio::test]
async fn assignment_rejects_a_binary_expression_destination() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let function_id = TargetID::TEST.make_global(SymbolID::default());
    let mut builder = TAstBuilder::new(engine, function_id);
    let a = literal(&mut builder, 1);
    let b = literal(&mut builder, 2);
    let c = literal(&mut builder, 3);
    let addition = builder.build_binary(a, BinaryOp::Plus, b);

    builder.build_binary(addition, BinaryOp::Assign, c);
    let (_, diagnostics) = builder.finish();

    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| { matches!(diagnostic, Diagnostic::ExpectedLvalue(_)) })
    );
}
