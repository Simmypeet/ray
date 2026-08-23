use rayc_parser::abstract_tree::AbstractTree;
use rayc_qbice::DuplicatingInterner;

use super::Type;

#[test]
fn immutable_pointer_type_is_parsed() {
    let tree = crate::test::parse_token_tree("*int32");
    let (ty, errors) = Type::parse(&tree, &DuplicatingInterner);
    let Type::Pointer(pointer) = ty.unwrap() else {
        panic!("expected a pointer type");
    };

    assert!(errors.is_empty());
    assert!(pointer.mut_keyword().is_none());
}

#[test]
fn mutable_pointer_type_is_parsed() {
    let tree = crate::test::parse_token_tree("*mut int32");
    let (ty, errors) = Type::parse(&tree, &DuplicatingInterner);
    let Type::Pointer(pointer) = ty.unwrap() else {
        panic!("expected a pointer type");
    };

    assert!(errors.is_empty());
    assert!(pointer.mut_keyword().is_some());
}

#[test]
fn polymorphic_variable_type_is_parsed() {
    let tree = crate::test::parse_token_tree("a");
    let (ty, errors) = Type::parse(&tree, &DuplicatingInterner);
    let Type::PolymorphicVariable(variable) = ty.unwrap() else {
        panic!("expected a polymorphic variable type");
    };

    assert!(errors.is_empty());
    assert_eq!(&*variable.kind.0, "a");
}
