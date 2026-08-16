use rayc_parser::abstract_tree::AbstractTree;
use rayc_qbice::DuplicatingInterner;

use super::{BinaryOperator, Expression, PostfixOperator};

#[test]
fn assignment_operator_is_parsed() {
    let tree = crate::test::parse_token_tree("a = b");
    let (expression, errors) = Expression::parse(&tree, &DuplicatingInterner);
    let Expression::Binary(binary) = expression.unwrap() else {
        panic!("expected a binary expression");
    };
    let subsequent = binary.subsequent().collect::<Vec<_>>();

    assert!(errors.is_empty());
    assert_eq!(subsequent.len(), 1);
    assert!(matches!(subsequent[0].operator(), Some(BinaryOperator::Assign(_))));
}

#[test]
fn mutable_reference_of_is_parsed() {
    let tree = crate::test::parse_token_tree("value.&mut");
    let (expression, errors) = Expression::parse(&tree, &DuplicatingInterner);
    let Expression::Binary(binary) = expression.unwrap() else {
        panic!("expected a binary expression");
    };
    let postfix = binary.postfix().unwrap();
    let postfixes = postfix.postfixes().collect::<Vec<_>>();

    assert!(errors.is_empty());
    assert_eq!(postfixes.len(), 1);
    assert!(matches!(
        &postfixes[0],
        PostfixOperator::RefOf(reference) if reference.mut_keyword().is_some()
    ));
}

#[test]
fn immutable_reference_of_is_parsed() {
    let tree = crate::test::parse_token_tree("value.&");
    let (expression, errors) = Expression::parse(&tree, &DuplicatingInterner);
    let Expression::Binary(binary) = expression.unwrap() else {
        panic!("expected a binary expression");
    };
    let postfix = binary.postfix().unwrap();
    let postfixes = postfix.postfixes().collect::<Vec<_>>();

    assert!(errors.is_empty());
    assert!(matches!(
        &postfixes[0],
        PostfixOperator::RefOf(reference) if reference.mut_keyword().is_none()
    ));
}
