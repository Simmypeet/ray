use rayc_arena::ID;
use rayc_lexical::tree::{Branch, OffsetMode, RelativeLocation, RelativeSpan};
use rayc_source_file::LocalSourceID;
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::ty::{Mutability, Primitive, Ty};
use rayc_typed_ast::{
    name_binding::{NameBinding, Source},
    typed_expr::{
        TypedExpr, TypedExprID, TypedExprKind, deref::Deref, identifier::Identifier,
        literal::Literal, paren::Paren, tuple_index::TupleIndex,
    },
    variable::Variable,
};

use crate::{
    diagnostic::{Diagnostic, LvalueOperation},
    tast_builder::TAstBuilder,
};

fn span() -> RelativeSpan {
    let location =
        RelativeLocation { offset: 0, mode: OffsetMode::Start, relative_to: ID::<Branch>::new(0) };

    RelativeSpan::new(location, location, TargetID::TEST.make_global(LocalSourceID::new(0, 0)))
}

async fn builder() -> TAstBuilder {
    TAstBuilder::new(
        rayc_qbice::create_minimal_engine().await,
        TargetID::TEST.make_global(SymbolID::default()),
    )
}

fn binding(builder: &mut TAstBuilder, mutable: bool) -> TypedExprID {
    let ty = Ty::new_primitive(Primitive::Int32, builder.engine());
    let variable = builder.insert_variable(Variable::new(ty.clone(), span()));
    let name_binding = builder.building_function.insert_name_binding(
        NameBinding::builder()
            .ty(ty.clone())
            .name(builder.engine().intern_unsized("value"))
            .source(Source::Variable(variable))
            .mutable(mutable)
            .span(span())
            .build(),
    );

    builder.insert_expression(TypedExpr::new(
        TypedExprKind::Identifier(Identifier::new(name_binding)),
        span(),
        ty,
    ))
}

fn dereference(builder: &mut TAstBuilder, mutability: Mutability) -> TypedExprID {
    let int32 = Ty::new_primitive(Primitive::Int32, builder.engine());
    let pointer_ty = Ty::new_pointer(int32.clone(), mutability, builder.engine());
    let pointer = builder.insert_expression(TypedExpr::new(
        TypedExprKind::Literal(Literal::Numeric(0)),
        span(),
        pointer_ty,
    ));

    builder.insert_expression(TypedExpr::new(
        TypedExprKind::Deref(Deref::new(pointer)),
        span(),
        int32,
    ))
}

#[tokio::test]
async fn mutable_requirement_distinguishes_binding_mutability() {
    let mut builder = builder().await;
    let mutable = binding(&mut builder, true);
    let immutable = binding(&mut builder, false);

    builder.require_lvalue(mutable, true, LvalueOperation::Assignment);
    builder.require_lvalue(immutable, true, LvalueOperation::Assignment);
    let (_, diagnostics) = builder.finish();

    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(diagnostic, Diagnostic::ImmutableLvalue(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn dereference_inherits_pointer_mutability() {
    let mut builder = builder().await;
    let mutable = dereference(&mut builder, Mutability::Mutable);
    let immutable = dereference(&mut builder, Mutability::Immutable);

    builder.require_lvalue(mutable, true, LvalueOperation::MutableReference);
    builder.require_lvalue(immutable, true, LvalueOperation::MutableReference);
    let (_, diagnostics) = builder.finish();

    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(diagnostic, Diagnostic::ImmutableLvalue(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn parenthesized_tuple_projection_inherits_its_base_lvalue() {
    let mut builder = builder().await;
    let mutable = binding(&mut builder, true);
    let projection = builder.insert_expression(TypedExpr::new(
        TypedExprKind::TupleIndex(TupleIndex::new(mutable, 0)),
        span(),
        Ty::new_primitive(Primitive::Int32, builder.engine()),
    ));
    let parenthesized = builder.insert_expression(TypedExpr::new(
        TypedExprKind::Paren(Paren::new(projection)),
        span(),
        Ty::new_primitive(Primitive::Int32, builder.engine()),
    ));

    builder.require_lvalue(parenthesized, true, LvalueOperation::Assignment);
    let (_, diagnostics) = builder.finish();

    assert!(diagnostics.is_empty());
}

#[tokio::test]
async fn literal_is_not_an_lvalue_but_needs_no_mutability_for_shared_reference() {
    let mut builder = builder().await;
    let literal = builder.insert_expression(TypedExpr::new(
        TypedExprKind::Literal(Literal::Numeric(1)),
        span(),
        Ty::new_primitive(Primitive::Int32, builder.engine()),
    ));
    let immutable = binding(&mut builder, false);

    builder.require_lvalue(literal, false, LvalueOperation::Reference);
    builder.require_lvalue(immutable, false, LvalueOperation::Reference);
    let (_, diagnostics) = builder.finish();

    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| matches!(diagnostic, Diagnostic::ExpectedLvalue(_)))
            .count(),
        1
    );
    assert!(
        !diagnostics
            .iter()
            .any(|diagnostic| { matches!(diagnostic, Diagnostic::ImmutableLvalue(_)) })
    );
}
