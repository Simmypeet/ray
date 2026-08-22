use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_semantic_element::parameter::Parameter;
use rayc_source_file::LocalSourceID;
use rayc_symbol::{GlobalSymbolID, SymbolID};
use rayc_target::TargetID;
use rayc_type::ty::{Primitive, Ty};

use crate::{
    address::{Address, AddressRoot, Projection},
    expression::{
        Expression, ExpressionKind,
        binary::{Binary, BinaryOp},
        call::Call,
        literal::Literal,
        load::Load,
        ref_of::RefOf,
        tuple::Tuple,
        tuple_index::TupleIndex,
    },
    function::Function,
    variable::Variable,
};

fn span(offset: usize) -> RelativeSpan {
    let location =
        RelativeLocation { offset, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
}

#[tokio::test]
async fn addresses_retain_each_root_and_tuple_projection() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let variable_id = ID::<Variable>::new(1);
    let parameter_id = ID::<Parameter>::new(2);
    let pointer_id = ID::<Expression>::new(3);
    let error = Address::new_error(&engine);
    let variable = Address::new_variable(variable_id, &engine);
    let parameter = Address::new_parameter(parameter_id, &engine);
    let dereference = Address::new_deref(pointer_id, &engine);
    let mut projected = variable.clone();

    projected.add_tuple_index(4, &engine);

    assert_eq!(error.root(), AddressRoot::Error);
    assert_eq!(variable.root(), AddressRoot::Variable(variable_id));
    assert_eq!(parameter.root(), AddressRoot::Parameter(parameter_id));
    assert_eq!(dereference.root(), AddressRoot::Deref(pointer_id));
    assert!(error.projections().is_empty());
    assert_eq!(projected.projections(), &[Projection::Tuple(4)]);
}

#[tokio::test]
async fn expression_payloads_round_trip_through_all_variants() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let ty = Ty::new_primitive(Primitive::Int32, &engine);
    let expression_span = span(1);
    let left = ID::<Expression>::new(0);
    let right = ID::<Expression>::new(1);
    let address = Address::new_error(&engine);
    let function_id = GlobalSymbolID::new(TargetID::TEST, SymbolID::from_u128(7));
    let expressions = [
        Expression::new_error(expression_span, ty.clone()),
        Expression::new(ExpressionKind::Literal(Literal::Numeric(42)), expression_span, ty.clone()),
        Expression::new(ExpressionKind::Literal(Literal::Bool(true)), expression_span, ty.clone()),
        Expression::new(
            ExpressionKind::RefOf(RefOf::new(address.clone())),
            expression_span,
            ty.clone(),
        ),
        Expression::new(
            ExpressionKind::Load(Load::new(address.clone())),
            expression_span,
            ty.clone(),
        ),
        Expression::new(
            ExpressionKind::Binary(Binary::new(left, BinaryOp::Plus, right)),
            expression_span,
            ty.clone(),
        ),
        Expression::new(
            ExpressionKind::Call(Call::new(function_id, vec![left, right])),
            expression_span,
            ty.clone(),
        ),
        Expression::new(
            ExpressionKind::Tuple(Tuple::new(vec![left, right])),
            expression_span,
            ty.clone(),
        ),
        Expression::new(
            ExpressionKind::TupleIndex(TupleIndex::new(left, 2)),
            expression_span,
            ty.clone(),
        ),
    ];

    assert!(matches!(expressions[0].kind(), ExpressionKind::Error));
    assert_eq!(expressions[0].span(), expression_span);
    assert_eq!(expressions[0].ty(), &ty);
    assert!(matches!(expressions[1].kind(), ExpressionKind::Literal(Literal::Numeric(42))));
    assert!(matches!(expressions[2].kind(), ExpressionKind::Literal(Literal::Bool(true))));

    let ExpressionKind::RefOf(reference) = expressions[3].kind() else {
        panic!("expected reference expression");
    };
    assert_eq!(reference.address(), &address);

    let ExpressionKind::Load(load) = expressions[4].kind() else {
        panic!("expected load expression");
    };
    assert_eq!(load.address(), &address);

    let ExpressionKind::Binary(binary) = expressions[5].kind() else {
        panic!("expected binary expression");
    };
    assert_eq!(binary.left(), left);
    assert_eq!(binary.operator(), BinaryOp::Plus);
    assert_eq!(binary.right(), right);

    let ExpressionKind::Call(call) = expressions[6].kind() else {
        panic!("expected call expression");
    };
    assert_eq!(call.function_id(), function_id);
    assert_eq!(call.arguments(), &[left, right]);

    let ExpressionKind::Tuple(tuple) = expressions[7].kind() else {
        panic!("expected tuple expression");
    };
    assert_eq!(tuple.elements(), &[left, right]);

    let ExpressionKind::TupleIndex(tuple_index) = expressions[8].kind() else {
        panic!("expected tuple-index expression");
    };
    assert_eq!(tuple_index.operand(), left);
    assert_eq!(tuple_index.index(), 2);
}

#[tokio::test]
async fn value_tuple_index_is_distinct_from_address_tuple_projection() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let operand = ID::<Expression>::new(0);
    let value_projection = TupleIndex::new(operand, 1);
    let mut address_projection = Address::new_error(&engine);

    address_projection.add_tuple_index(1, &engine);

    assert_eq!(value_projection.operand(), operand);
    assert_eq!(value_projection.index(), 1);
    assert_eq!(address_projection.projections(), &[Projection::Tuple(1)]);
}

#[tokio::test]
async fn compiler_generated_expression_keeps_enclosing_expression_span() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let enclosing_span = span(9);
    let bool_ty = Ty::new_primitive(Primitive::Bool, &engine);
    let generated =
        Expression::new(ExpressionKind::Literal(Literal::Bool(false)), enclosing_span, bool_ty);

    assert_eq!(generated.span(), enclosing_span);
}

#[tokio::test]
async fn function_inserts_and_inspects_finalized_expression_forms() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let ty = Ty::new_primitive(Primitive::Int32, &engine);
    let expression_span = span(3);
    let mut function = Function::new();
    let expression_id = function.insert_expression(Expression::new(
        ExpressionKind::Literal(Literal::Numeric(5)),
        expression_span,
        ty,
    ));

    assert!(matches!(
        function.get_expression(expression_id).kind(),
        ExpressionKind::Literal(Literal::Numeric(5))
    ));
}

#[test]
fn binary_operators_are_limited_to_arithmetic_forms() {
    const fn symbol(operator: BinaryOp) -> &'static str {
        match operator {
            BinaryOp::Plus => "+",
            BinaryOp::Minus => "-",
            BinaryOp::Multiply => "*",
            BinaryOp::Divide => "/",
        }
    }

    assert_eq!(symbol(BinaryOp::Plus), "+");
    assert_eq!(symbol(BinaryOp::Minus), "-");
    assert_eq!(symbol(BinaryOp::Multiply), "*");
    assert_eq!(symbol(BinaryOp::Divide), "/");
}
