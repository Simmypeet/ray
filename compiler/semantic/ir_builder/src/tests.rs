use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::AddressRoot,
    cfg::{Instruction, Terminator},
    expression::{ExpressionKind, make_lambda::MakeLambda},
    function::Function as IrFunction,
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::TrackedEngine;
use rayc_source_file::{GlobalSourceID, LocalSourceID};
use rayc_target::TargetID;
use rayc_type::ty::{Mutability, Primitive, Ty, TyApplicationView};
use rayc_typed_ast::{
    lambda::LambdaParameter,
    name_binding::{NameBinding, Source},
    statement::{Let, Statement},
    typed_expr::{
        TypedExpr, TypedExprID, TypedExprKind,
        binary::{Binary, BinaryOp},
        identifier::Identifier,
        lambda::Lambda,
        literal::Literal,
    },
    typed_function::{FunctionID, FunctionLocalID, TypedFunctionMap},
    variable::Variable,
};

use crate::lower_function;

struct TestMap {
    functions: TypedFunctionMap,
    int_ty: Interned<Ty>,
    unit_ty: Interned<Ty>,
    next_span: usize,
    bindings: FxHashMap<Source, rayc_typed_ast::name_binding::NameBindingID>,
}

impl TestMap {
    fn new(engine: &TrackedEngine) -> Self {
        Self {
            functions: TypedFunctionMap::default(),
            int_ty: Ty::new_primitive(Primitive::Int32, engine),
            unit_ty: Ty::new_unit(engine),
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

    fn variable(&mut self, owner: FunctionID, name: &'static str) -> Source {
        let span = self.span();
        let variable =
            self.functions.insert_variable_into(owner, Variable::new(self.int_ty.clone(), span));
        let source = Source::Variable(FunctionLocalID::new(owner, variable));
        self.insert_binding(source, name, span);
        source
    }

    fn initialize_variable(&mut self, owner: FunctionID, source: Source, value: TypedExprID) {
        let Source::Variable(variable) = source else {
            panic!("only variables can be initialized by a let statement");
        };
        assert_eq!(variable.function_id(), owner);
        let name_binding_group_id = self.functions.new_name_binding_group();
        let span = self.span();
        self.functions.push_statement_into(
            owner,
            Statement::Let(
                Let::builder()
                    .variable_id(variable.local_id())
                    .name_binding_group_id(name_binding_group_id)
                    .expression(value)
                    .span(span)
                    .build(),
            ),
        );
    }

    fn lambda_parameter(&mut self, owner: FunctionID, name: &'static str) -> Source {
        let span = self.span();
        let parameter = self
            .functions
            .insert_lambda_parameter(owner, LambdaParameter::new(self.int_ty.clone(), span));
        let source = Source::LambdaParameter(FunctionLocalID::new(owner, parameter));
        self.insert_binding(source, name, span);
        source
    }

    fn insert_binding(&mut self, source: Source, name: &'static str, span: RelativeSpan) {
        let binding = NameBinding::builder()
            .ty(self.int_ty.clone())
            .name(Interned::new_duplicating_unsized(name))
            .source(source)
            .mutable(true)
            .span(span)
            .build();
        let binding_id = self.functions.insert_name_binding(binding);
        assert!(self.bindings.insert(source, binding_id).is_none());
    }

    fn expression(
        &mut self,
        owner: FunctionID,
        kind: TypedExprKind,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        let span = self.span();
        self.functions.insert_expression_into(owner, TypedExpr::new(kind, span, ty))
    }

    fn literal(&mut self, owner: FunctionID, value: u128) -> TypedExprID {
        self.expression(owner, TypedExprKind::Literal(Literal::Numeric(value)), self.int_ty.clone())
    }

    fn identifier(&mut self, owner: FunctionID, source: Source) -> TypedExprID {
        let binding_id = *self.bindings.get(&source).expect("test binding should exist");
        self.expression(
            owner,
            TypedExprKind::Identifier(Identifier::new(binding_id)),
            self.int_ty.clone(),
        )
    }

    fn assignment(
        &mut self,
        owner: FunctionID,
        destination: TypedExprID,
        value: TypedExprID,
    ) -> TypedExprID {
        self.expression(
            owner,
            TypedExprKind::Binary(Binary::new(destination, BinaryOp::Assign, value)),
            self.int_ty.clone(),
        )
    }

    fn lambda_expression(
        &mut self,
        owner: FunctionID,
        child: FunctionID,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        self.expression(owner, TypedExprKind::Lambda(Lambda::new(child)), ty)
    }

    fn statement(&mut self, owner: FunctionID, expression: TypedExprID) {
        self.functions.push_statement_into(owner, Statement::Expression(expression));
    }
}

fn make_lambdas(function: &IrFunction) -> Vec<&MakeLambda> {
    function
        .reachables()
        .expressions()
        .filter_map(|id| match function.get_expression(id).kind() {
            ExpressionKind::MakeLambda(lambda) => Some(lambda),
            ExpressionKind::Error
            | ExpressionKind::Literal(_)
            | ExpressionKind::RefOf(_)
            | ExpressionKind::Load(_)
            | ExpressionKind::Phi(_)
            | ExpressionKind::Binary(_)
            | ExpressionKind::Call(_)
            | ExpressionKind::Tuple(_) => None,
        })
        .collect()
}

fn lambda_context(function: &IrFunction) -> &rayc_ir::lambda::LambdaContext {
    function.context().assert_as_lambda_context()
}

fn pointer_mutability(ty: &Ty) -> Mutability {
    match ty {
        Ty::Application(application) => match application.view() {
            TyApplicationView::Pointer(pointer) => pointer.mutability(),
            TyApplicationView::Primitive(_)
            | TyApplicationView::Tuple(_)
            | TyApplicationView::Lambda(_)
            | TyApplicationView::Error => panic!("expected a pointer type"),
        },
        Ty::Inference(_) | Ty::PolyVar(_) => panic!("expected a concrete pointer type"),
    }
}

#[tokio::test]
async fn captureless_lambda_copies_signature_and_uses_lambda_parameter_addresses() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    let child = map.functions.insert_lambda();
    let parameter = map.lambda_parameter(child, "value");
    let parameter_read = map.identifier(child, parameter);
    map.statement(child, parameter_read);
    let lambda_ty = Ty::new_lambda([map.int_ty.clone()], map.unit_ty.clone(), &engine);
    let lambda = map.lambda_expression(root, child, lambda_ty);
    map.statement(root, lambda);

    let ir = lower_function(&engine, &map.functions);
    let root_lambdas = make_lambdas(ir.root());
    assert_eq!(root_lambdas.len(), 1);
    assert!(root_lambdas[0].captures().is_empty());

    let child = ir.get_function(root_lambdas[0].function_id());
    let context = lambda_context(child);
    let parameters: Vec<_> = context.parameters().collect();
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0].1.ty(), &map.int_ty);
    assert_eq!(context.return_ty(), &map.unit_ty);

    let read = child.reachables().expressions().next().expect("parameter should be read");
    let ExpressionKind::Load(load) = child.get_expression(read).kind() else {
        panic!("lambda parameter read should lower to a load");
    };
    assert_eq!(load.address().root(), AddressRoot::LambdaParameter(parameters[0].0));
}

#[tokio::test]
async fn mutable_capture_is_passed_by_reference_and_written_through_its_pointer() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    let initial = map.literal(root, 0);
    let captured = map.variable(root, "captured");
    map.initialize_variable(root, captured, initial);
    let child = map.functions.insert_lambda();
    let destination = map.identifier(child, captured);
    let value = map.literal(child, 1);
    let assignment = map.assignment(child, destination, value);
    map.statement(child, assignment);
    let lambda_ty = Ty::new_lambda([], map.unit_ty.clone(), &engine);
    let lambda = map.lambda_expression(root, child, lambda_ty);
    map.statement(root, lambda);

    let ir = lower_function(&engine, &map.functions);
    let root_lambdas = make_lambdas(ir.root());
    assert_eq!(root_lambdas.len(), 1);
    let make_lambda = root_lambdas[0];
    assert_eq!(make_lambda.captures().len(), 1);
    let capture_operand = ir.root().get_expression(make_lambda.captures()[0]);
    assert_eq!(pointer_mutability(capture_operand.ty()), Mutability::Mutable);
    let ExpressionKind::RefOf(reference) = capture_operand.kind() else {
        panic!("closure capture operand should be a reference");
    };
    assert!(matches!(reference.address().root(), AddressRoot::Variable(_)));

    let child = ir.get_function(make_lambda.function_id());
    let context = lambda_context(child);
    let capture_layout: Vec<_> = context.captures().collect();
    assert_eq!(capture_layout.len(), 1);
    let (capture_id, capture) = capture_layout[0];
    assert_eq!(capture.mutability(), Mutability::Mutable);

    let store = child
        .block_instructions(child.entry_block())
        .iter()
        .find_map(|instruction| match instruction {
            Instruction::Expression(_) => None,
            Instruction::Store(store) => Some(store),
        })
        .expect("assignment should emit a store");
    let AddressRoot::Deref(pointer) = store.address().root() else {
        panic!("captured assignment should dereference the capture pointer");
    };
    let ExpressionKind::Load(load) = child.get_expression(pointer).kind() else {
        panic!("captured assignment should load its capture pointer");
    };
    assert_eq!(load.address().root(), AddressRoot::Capture(capture_id));
}

#[tokio::test]
async fn nested_lambdas_reborrow_a_transitive_capture_with_each_childs_mutability() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    let initial = map.literal(root, 0);
    let captured = map.variable(root, "captured");
    map.initialize_variable(root, captured, initial);

    let outer = map.functions.insert_lambda();
    let reader = map.functions.insert_lambda();
    let writer = map.functions.insert_lambda();
    let read = map.identifier(reader, captured);
    map.statement(reader, read);
    let destination = map.identifier(writer, captured);
    let value = map.literal(writer, 1);
    let assignment = map.assignment(writer, destination, value);
    map.statement(writer, assignment);

    let child_ty = Ty::new_lambda([], map.unit_ty.clone(), &engine);
    let reader_lambda = map.lambda_expression(outer, reader, child_ty.clone());
    map.statement(outer, reader_lambda);
    let writer_lambda = map.lambda_expression(outer, writer, child_ty);
    map.statement(outer, writer_lambda);
    let outer_ty = Ty::new_lambda([], map.unit_ty.clone(), &engine);
    let outer_lambda = map.lambda_expression(root, outer, outer_ty);
    map.statement(root, outer_lambda);

    let ir = lower_function(&engine, &map.functions);
    let root_lambdas = make_lambdas(ir.root());
    assert_eq!(root_lambdas.len(), 1);
    let outer = ir.get_function(root_lambdas[0].function_id());
    let outer_context = lambda_context(outer);
    let outer_captures: Vec<_> = outer_context.captures().collect();
    assert_eq!(outer_captures.len(), 1);
    assert_eq!(outer_captures[0].1.mutability(), Mutability::Mutable);

    let child_lambdas = make_lambdas(outer);
    assert_eq!(child_lambdas.len(), 2);
    for (make_lambda, expected_mutability) in
        child_lambdas.into_iter().zip([Mutability::Immutable, Mutability::Mutable])
    {
        let child = ir.get_function(make_lambda.function_id());
        let child_captures: Vec<_> = lambda_context(child).captures().collect();
        assert_eq!(child_captures.len(), 1);
        assert_eq!(child_captures[0].1.mutability(), expected_mutability);
        assert_eq!(make_lambda.captures().len(), 1);

        let reborrow = outer.get_expression(make_lambda.captures()[0]);
        assert_eq!(pointer_mutability(reborrow.ty()), expected_mutability);
        let ExpressionKind::RefOf(reference) = reborrow.kind() else {
            panic!("forwarded capture should be explicitly reborrowed");
        };
        let AddressRoot::Deref(pointer) = reference.address().root() else {
            panic!("forwarded capture should reborrow the captured pointee");
        };
        let ExpressionKind::Load(load) = outer.get_expression(pointer).kind() else {
            panic!("forwarded capture should load the parent capture pointer");
        };
        assert_eq!(load.address().root(), AddressRoot::Capture(outer_captures[0].0));
    }

    assert!(matches!(outer.block_terminator(outer.entry_block()), Some(Terminator::Return(None))));
}
