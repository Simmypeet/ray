use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_source_file::{GlobalSourceID, LocalSourceID};
use rayc_target::TargetID;
use rayc_type::ty::{Mutability, Ty, TyInference, TyKind};
use rayc_typed_ast::{
    name_binding::{NameBinding, Source},
    statement::Statement,
    typed_expr::{
        TypedExpr, TypedExprID, TypedExprKind,
        binary::{Binary, BinaryOp},
        deref::Deref,
        identifier::Identifier,
        lambda::Lambda,
        literal::Literal,
        paren::Paren,
        ref_of::RefOf,
        tuple::Tuple,
        tuple_index::TupleIndex,
    },
    typed_function::{TypedFunctionID, TypedFunctionLocalID, TypedFunctionMap},
    typed_lambda::TypedLambdaParameter,
    typed_variable::TypedVariable,
};

use super::CaptureAnalysis;

struct TestMap {
    functions: TypedFunctionMap,
    ty: Interned<Ty>,
    next_span: usize,
    bindings: FxHashMap<Source, rayc_typed_ast::name_binding::NameBindingID>,
}

impl TestMap {
    fn new() -> Self {
        Self {
            functions: TypedFunctionMap::default(),
            ty: Interned::new_duplicating(Ty::Inference(TyInference::new(TyKind::Star, 0))),
            next_span: 0,
            bindings: FxHashMap::default(),
        }
    }

    fn span(&mut self) -> RelativeSpan {
        let offset = self.next_span;
        self.next_span += 1;
        let source_id: GlobalSourceID = TargetID::TEST.make_global(LocalSourceID::new(0, 0));
        RelativeSpan {
            start: RelativeLocation {
                offset,
                mode: OffsetMode::Start,
                relative_to: ROOT_BRANCH_ID,
            },
            end: RelativeLocation {
                offset: offset + 1,
                mode: OffsetMode::End,
                relative_to: ROOT_BRANCH_ID,
            },
            source_id,
        }
    }

    fn variable(&mut self, owner: TypedFunctionID, name: &'static str) -> Source {
        let span = self.span();
        let variable =
            self.functions.insert_variable(owner, TypedVariable::new(self.ty.clone(), span));
        let source = Source::Variable(TypedFunctionLocalID::new(owner, variable));
        self.insert_binding(source, name, span);
        source
    }

    fn lambda_parameter(&mut self, owner: TypedFunctionID, name: &'static str) -> Source {
        let span = self.span();
        let parameter = self
            .functions
            .insert_lambda_parameter(owner, TypedLambdaParameter::new(self.ty.clone(), span));
        let source = Source::LambdaParameter(TypedFunctionLocalID::new(owner, parameter));
        self.insert_binding(source, name, span);
        source
    }

    fn insert_binding(&mut self, source: Source, name: &'static str, span: RelativeSpan) {
        let binding = NameBinding::builder()
            .ty(self.ty.clone())
            .name(Interned::new_duplicating_unsized(name))
            .source(source)
            .mutable(true)
            .span(span)
            .build();
        let binding_id = self.functions.insert_name_binding(binding);
        assert!(self.bindings.insert(source, binding_id).is_none());
    }

    fn binding_id(&self, source: Source) -> rayc_typed_ast::name_binding::NameBindingID {
        *self.bindings.get(&source).expect("test source should have a name binding")
    }

    fn identifier(&mut self, function: TypedFunctionID, source: Source) -> TypedExprID {
        self.expression(
            function,
            TypedExprKind::Identifier(Identifier::new(self.binding_id(source))),
        )
    }

    fn expression(&mut self, function: TypedFunctionID, kind: TypedExprKind) -> TypedExprID {
        let span = self.span();
        self.functions.insert_expression(function, TypedExpr::new(kind, span, self.ty.clone()))
    }

    fn statement(&mut self, function: TypedFunctionID, expression: TypedExprID) {
        self.functions.push_statement(function, Statement::Expression(expression));
    }

    fn lambda_expression(
        &mut self,
        parent: TypedFunctionID,
        child: TypedFunctionID,
    ) -> TypedExprID {
        self.expression(parent, TypedExprKind::Lambda(Lambda::new(child)))
    }
}

#[test]
fn bindings_owned_by_the_current_function_are_not_captured() {
    let mut map = TestMap::new();
    let root = map.functions.root_id();
    let child = map.functions.insert_lambda();
    let local = map.variable(child, "local");
    let parameter = map.lambda_parameter(child, "parameter");
    let local = map.identifier(child, local);
    let parameter = map.identifier(child, parameter);
    let tuple = map.expression(child, TypedExprKind::Tuple(Tuple::new(vec![local, parameter])));
    map.statement(child, tuple);
    let lambda = map.lambda_expression(root, child);
    map.statement(root, lambda);

    let analysis = CaptureAnalysis::analyze(&map.functions);

    assert_eq!(analysis.plan(child).captures().len(), 0);
    assert_eq!(analysis.plan(root).captures().len(), 0);
}

#[test]
fn repeated_uses_keep_first_encounter_order_and_upgrade_mutability_in_place() {
    let mut map = TestMap::new();
    let root = map.functions.root_id();
    let first = map.variable(root, "first");
    let second = map.variable(root, "second");
    let child = map.functions.insert_lambda();

    let first_read = map.identifier(child, first);
    map.statement(child, first_read);
    let second_read = map.identifier(child, second);
    map.statement(child, second_read);
    let first_write = map.identifier(child, first);
    let value = map.expression(child, TypedExprKind::Literal(Literal::Numeric(1)));
    let assignment = map.expression(
        child,
        TypedExprKind::Binary(Binary::new(first_write, BinaryOp::Assign, value)),
    );
    map.statement(child, assignment);
    let lambda = map.lambda_expression(root, child);
    map.statement(root, lambda);

    let analysis = CaptureAnalysis::analyze(&map.functions);
    let captures: Vec<_> = analysis.plan(child).captures().collect();

    assert_eq!(captures.len(), 2);
    assert_eq!(captures[0].1.source(), first);
    assert_eq!(captures[0].1.pointee_ty(), &map.ty);
    assert_eq!(captures[0].1.span(), *map.functions.get_name_binding(map.binding_id(first)).span());
    assert_eq!(captures[0].1.mutability(), Mutability::Mutable);
    assert_eq!(captures[1].1.source(), second);
    assert_eq!(captures[1].1.mutability(), Mutability::Immutable);
}

#[test]
fn address_modes_follow_projections_references_and_dereferences() {
    let mut map = TestMap::new();
    let root = map.functions.root_id();
    let projected = map.variable(root, "projected");
    let referenced = map.variable(root, "referenced");
    let pointer = map.variable(root, "pointer");
    let child = map.functions.insert_lambda();

    let projected_id = map.identifier(child, projected);
    let parenthesized = map.expression(child, TypedExprKind::Paren(Paren::new(projected_id)));
    let projection =
        map.expression(child, TypedExprKind::TupleIndex(TupleIndex::new(parenthesized, 0)));
    let value = map.expression(child, TypedExprKind::Literal(Literal::Numeric(1)));
    let projection_assignment = map
        .expression(child, TypedExprKind::Binary(Binary::new(projection, BinaryOp::Assign, value)));
    map.statement(child, projection_assignment);

    let referenced_id = map.identifier(child, referenced);
    let mutable_reference =
        map.expression(child, TypedExprKind::RefOf(RefOf::new(referenced_id, Mutability::Mutable)));
    map.statement(child, mutable_reference);

    let pointer_id = map.identifier(child, pointer);
    let dereference = map.expression(child, TypedExprKind::Deref(Deref::new(pointer_id)));
    let value = map.expression(child, TypedExprKind::Literal(Literal::Numeric(2)));
    let dereference_assignment = map.expression(
        child,
        TypedExprKind::Binary(Binary::new(dereference, BinaryOp::Assign, value)),
    );
    map.statement(child, dereference_assignment);
    let lambda = map.lambda_expression(root, child);
    map.statement(root, lambda);

    let analysis = CaptureAnalysis::analyze(&map.functions);
    let captures: Vec<_> = analysis
        .plan(child)
        .captures()
        .map(|(_, capture)| (capture.source(), capture.mutability()))
        .collect();

    assert_eq!(captures, vec![
        (projected, Mutability::Mutable),
        (referenced, Mutability::Mutable),
        (pointer, Mutability::Immutable),
    ]);
}

#[test]
fn nested_children_propagate_only_ancestor_captures_with_joined_mutability() {
    let mut map = TestMap::new();
    let root = map.functions.root_id();
    let ancestor = map.variable(root, "ancestor");
    let outer = map.functions.insert_lambda();
    let parent_local = map.variable(outer, "parent_local");
    let reader = map.functions.insert_lambda();
    let writer = map.functions.insert_lambda();

    let ancestor_read = map.identifier(reader, ancestor);
    map.statement(reader, ancestor_read);
    let parent_local_write = map.identifier(reader, parent_local);
    let value = map.expression(reader, TypedExprKind::Literal(Literal::Numeric(1)));
    let assignment = map.expression(
        reader,
        TypedExprKind::Binary(Binary::new(parent_local_write, BinaryOp::Assign, value)),
    );
    map.statement(reader, assignment);

    let ancestor_write = map.identifier(writer, ancestor);
    let value = map.expression(writer, TypedExprKind::Literal(Literal::Numeric(2)));
    let assignment = map.expression(
        writer,
        TypedExprKind::Binary(Binary::new(ancestor_write, BinaryOp::Assign, value)),
    );
    map.statement(writer, assignment);

    let reader_lambda = map.lambda_expression(outer, reader);
    map.statement(outer, reader_lambda);
    let writer_lambda = map.lambda_expression(outer, writer);
    map.statement(outer, writer_lambda);
    let outer_lambda = map.lambda_expression(root, outer);
    map.statement(root, outer_lambda);

    let analysis = CaptureAnalysis::analyze(&map.functions);
    let reader_captures: Vec<_> = analysis
        .plan(reader)
        .captures()
        .map(|(_, capture)| (capture.source(), capture.mutability()))
        .collect();
    let writer_captures: Vec<_> = analysis
        .plan(writer)
        .captures()
        .map(|(_, capture)| (capture.source(), capture.mutability()))
        .collect();
    let outer_captures: Vec<_> = analysis
        .plan(outer)
        .captures()
        .map(|(_, capture)| (capture.source(), capture.mutability()))
        .collect();

    assert_eq!(reader_captures, vec![
        (ancestor, Mutability::Immutable),
        (parent_local, Mutability::Mutable)
    ]);
    assert_eq!(writer_captures, vec![(ancestor, Mutability::Mutable)]);
    assert_eq!(outer_captures, vec![(ancestor, Mutability::Mutable)]);
    assert_eq!(analysis.plan(root).captures().len(), 0);
}
