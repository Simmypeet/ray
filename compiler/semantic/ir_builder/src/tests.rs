use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::AddressRoot,
    cfg::Instruction,
    ir_expr::{IRExprKind, closure::Closure as IrClosure},
    ir_function::IRFunction as IrFunction,
};
use rayc_lexical::tree::{OffsetMode, ROOT_BRANCH_ID, RelativeLocation, RelativeSpan};
use rayc_qbice::TrackedEngine;
use rayc_source_file::{GlobalSourceID, LocalSourceID};
use rayc_symbol::SymbolID;
use rayc_target::TargetID;
use rayc_type::{
    subst::Subst,
    ty::{Mutability, Primitive, Ty, effect_row::EffectLabel},
};
use rayc_typed_ast::{
    capture_plan::CapturePlan,
    name_binding::{NameBinding, Source},
    statement::{Let, Statement},
    typed_expr::{
        TypedExpr, TypedExprID, TypedExprKind,
        binary::{Binary, BinaryOp},
        call::Call,
        closure::Closure,
        identifier::Identifier,
        literal::Literal,
        run_with::RunWith,
    },
    typed_function::{TypedFunctionID, TypedFunctionLocalID, TypedFunctionMap},
    typed_lambda::TypedLambdaParameter,
    typed_operation_handler::TypedOperationHandlerParameter,
    typed_variable::TypedVariable,
};

use crate::lower_function;

struct TestMap {
    functions: TypedFunctionMap,
    int_ty: Interned<Ty>,
    unit_ty: Interned<Ty>,
    effect: Interned<Ty>,
    next_span: usize,
    bindings: FxHashMap<Source, rayc_typed_ast::name_binding::NameBindingID>,
}

impl TestMap {
    fn new(engine: &TrackedEngine) -> Self {
        let effect = Ty::new_effect_row([], None, engine);
        Self {
            functions: TypedFunctionMap::new(effect.clone()),
            int_ty: Ty::new_primitive(Primitive::Int32, engine),
            unit_ty: Ty::new_unit(engine),
            effect,
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
            self.functions.insert_variable(owner, TypedVariable::new(self.int_ty.clone(), span));
        let source = Source::Variable(TypedFunctionLocalID::new(owner, variable));
        self.insert_binding(source, name, span);
        source
    }

    fn initialize_variable(&mut self, owner: TypedFunctionID, source: Source, value: TypedExprID) {
        let Source::Variable(variable) = source else {
            panic!("only variables can be initialized by a let statement");
        };
        assert_eq!(variable.function_id(), owner);
        let name_binding_group_id = self.functions.new_name_binding_group();
        let span = self.span();
        self.functions.push_statement(
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

    fn lambda_parameter(&mut self, owner: TypedFunctionID, name: &'static str) -> Source {
        let span = self.span();
        let parameter = self
            .functions
            .insert_lambda_parameter(owner, TypedLambdaParameter::new(self.int_ty.clone(), span));
        let source = Source::LambdaParameter(TypedFunctionLocalID::new(owner, parameter));
        self.insert_binding(source, name, span);
        source
    }

    fn operation_handler_parameter(
        &mut self,
        owner: TypedFunctionID,
        name: &'static str,
    ) -> Source {
        let span = self.span();
        let parameter = self.functions.insert_operation_handler_parameter(
            owner,
            TypedOperationHandlerParameter::new(self.int_ty.clone(), span),
        );
        let source = Source::OperationHandlerParameter(TypedFunctionLocalID::new(owner, parameter));
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
        owner: TypedFunctionID,
        kind: TypedExprKind,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        let span = self.span();
        self.functions.insert_expression(owner, TypedExpr::new(kind, span, ty, self.effect.clone()))
    }

    fn lambda(&mut self) -> TypedFunctionID { self.functions.insert_lambda(self.effect.clone()) }

    fn literal(&mut self, owner: TypedFunctionID, value: u128) -> TypedExprID {
        self.expression(owner, TypedExprKind::Literal(Literal::Numeric(value)), self.int_ty.clone())
    }

    fn identifier(&mut self, owner: TypedFunctionID, source: Source) -> TypedExprID {
        let binding_id = *self.bindings.get(&source).expect("test binding should exist");
        self.expression(
            owner,
            TypedExprKind::Identifier(Identifier::new(binding_id)),
            self.int_ty.clone(),
        )
    }

    fn assignment(
        &mut self,
        owner: TypedFunctionID,
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
        engine: &TrackedEngine,
        owner: TypedFunctionID,
        child: TypedFunctionID,
        ty: Interned<Ty>,
    ) -> TypedExprID {
        let closure_id = self.functions.register_closure(child);
        let source_owner = TargetID::TEST.make_global(rayc_symbol::SymbolID::from_u128(1));
        let parameter_types = self
            .functions
            .get_function(child)
            .context()
            .assert_as_lambda_context()
            .parameters()
            .map(|(_, parameter)| parameter.ty().clone());
        let ty = Ty::new_closure(
            rayc_type::ty::application::Closure::new(source_owner, closure_id, 0),
            [],
            parameter_types,
            ty,
            self.effect.clone(),
            self.unit_ty.clone(),
            engine,
        );
        self.expression(owner, TypedExprKind::Closure(Closure::new(child)), ty)
    }

    fn statement(&mut self, owner: TypedFunctionID, expression: TypedExprID) {
        self.functions.push_statement(owner, Statement::Expression(expression));
    }
}

fn make_lambdas(function: &IrFunction) -> Vec<&IrClosure> {
    function
        .reachables()
        .expressions()
        .filter_map(|id| match function.get_expression(id).kind() {
            IRExprKind::Closure(lambda) => Some(lambda),
            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Call(_)
            | IRExprKind::Perform(_)
            | IRExprKind::Handle(_)
            | IRExprKind::Tuple(_) => None,
        })
        .collect()
}

fn lambda_context(function: &IrFunction) -> &rayc_ir::ir_lambda::IRLambdaContext {
    function.context().assert_as_lambda_context()
}

#[tokio::test]
async fn nominal_closure_ids_resolve_to_nested_lowered_functions() {
    use rayc_type::ty::application::Closure;
    use rayc_typed_ast::{TypedAst, typed_expr::closure::Closure as TypedClosure};

    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    // Allocate the inner function first so lowering cannot reuse the typed IDs.
    let inner = map.lambda();
    let outer = map.lambda();
    let inner_id = map.functions.register_closure(inner);
    let outer_id = map.functions.register_closure(outer);
    let owner = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let inner_ty = Ty::new_closure(
        Closure::new(owner, inner_id, 0),
        [],
        [],
        map.unit_ty.clone(),
        map.effect.clone(),
        map.unit_ty.clone(),
        &engine,
    );
    let inner_expr =
        map.expression(outer, TypedExprKind::Closure(TypedClosure::new(inner)), inner_ty);
    map.statement(outer, inner_expr);
    let outer_ty = Ty::new_closure(
        Closure::new(owner, outer_id, 0),
        [],
        [],
        map.unit_ty.clone(),
        map.effect.clone(),
        map.unit_ty.clone(),
        &engine,
    );
    let outer_expr =
        map.expression(root, TypedExprKind::Closure(TypedClosure::new(outer)), outer_ty);
    map.statement(root, outer_expr);
    let captures = CapturePlan::analyze(&map.functions);
    let ast = TypedAst::new(map.functions, captures);

    let (ir, diagnostics) =
        lower_function(&engine, ast.functions(), ast.captures(), map.unit_ty, None);

    assert!(diagnostics.is_empty());
    assert_eq!(ast.closure_function(inner_id), Some(inner));
    assert_eq!(ast.closure_function(outer_id), Some(outer));
    let outer_expression_id = ir.root().reachables().expressions().next().unwrap();
    let IRExprKind::Closure(outer_lambda) = ir.root().get_expression(outer_expression_id).kind()
    else {
        panic!("nominal closures should lower to Closure");
    };
    let ir_outer = outer_lambda.function_id();
    let outer_function = ir.get_function(ir_outer);
    let inner_expression_id = outer_function.reachables().expressions().next().unwrap();
    let IRExprKind::Closure(inner_lambda) =
        outer_function.get_expression(inner_expression_id).kind()
    else {
        panic!("nested nominal closures should lower to Closure");
    };
    let ir_inner = inner_lambda.function_id();
    assert!(outer_lambda.captures().is_empty());
    assert!(inner_lambda.captures().is_empty());
    assert_ne!(ir_outer.index(), outer.index());
    assert_eq!(ir.closure_function(outer_id), Some(ir_outer));
    assert_eq!(ir.closure_function(inner_id), Some(ir_inner));
    assert_eq!(ir.closures().len(), 2);
}

#[tokio::test]
async fn captureless_lambda_copies_signature_and_uses_lambda_parameter_addresses() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    let child = map.lambda();
    let parameter = map.lambda_parameter(child, "value");
    let parameter_read = map.identifier(child, parameter);
    map.statement(child, parameter_read);
    let lambda_ty = map.unit_ty.clone();
    let lambda = map.lambda_expression(&engine, root, child, lambda_ty);
    map.statement(root, lambda);

    let (ir, diagnostics) = lower_function(
        &engine,
        &map.functions,
        &CapturePlan::analyze(&map.functions),
        map.unit_ty.clone(),
        None,
    );
    assert!(diagnostics.is_empty());
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
    let IRExprKind::Load(load) = child.get_expression(read).kind() else {
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
    let child = map.lambda();
    let destination = map.identifier(child, captured);
    let value = map.literal(child, 1);
    let assignment = map.assignment(child, destination, value);
    map.statement(child, assignment);
    let lambda_ty = map.unit_ty.clone();
    let lambda = map.lambda_expression(&engine, root, child, lambda_ty);
    map.statement(root, lambda);

    let (ir, diagnostics) = lower_function(
        &engine,
        &map.functions,
        &CapturePlan::analyze(&map.functions),
        map.unit_ty.clone(),
        None,
    );
    assert!(diagnostics.is_empty());
    let root_lambdas = make_lambdas(ir.root());
    assert_eq!(root_lambdas.len(), 1);
    let make_lambda = root_lambdas[0];
    assert_eq!(make_lambda.captures().len(), 1);
    let capture_operand = ir.root().get_expression(make_lambda.captures()[0]);
    assert_eq!(capture_operand.ty().as_pointer_mutability().unwrap(), Mutability::Mutable);
    let IRExprKind::RefOf(reference) = capture_operand.kind() else {
        panic!("closure capture operand should be a reference");
    };
    assert!(matches!(reference.address().root(), AddressRoot::Variable(_)));

    let child = ir.get_function(make_lambda.function_id());
    let capture_layout: Vec<_> = ir.captures(make_lambda.function_id()).collect();
    assert_eq!(capture_layout.len(), 1);
    let (capture_id, capture) = capture_layout[0];
    assert_eq!(capture.mode(), rayc_type::capture::CaptureMode::Reference(Mutability::Mutable));

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
    let IRExprKind::Load(load) = child.get_expression(pointer).kind() else {
        panic!("captured assignment should load its capture pointer");
    };
    assert_eq!(load.address().root(), AddressRoot::Capture(capture_id));
}

#[tokio::test]
async fn effect_operation_call_lowers_to_perform_and_preserves_function_effect() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let effect_id = TargetID::TEST.make_global(SymbolID::from_u128(1));
    let operation_id = TargetID::TEST.make_global(SymbolID::from_u128(2));
    let effect_label =
        engine.intern(EffectLabel::new(effect_id, rayc_type::ty::args::Args::new([], &engine)));
    let effect = Ty::new_effect_row([effect_label], None, &engine);
    let mut map = TestMap::new(&engine);
    map.effect = effect.clone();
    map.functions = TypedFunctionMap::new(effect.clone());
    let root = map.functions.root_id();
    let argument = map.literal(root, 7);
    let perform = map.expression(
        root,
        TypedExprKind::Call(Call::new_effect_operation(
            effect_id,
            operation_id,
            vec![argument],
            Subst::new_empty(),
        )),
        map.int_ty.clone(),
    );
    map.statement(root, perform);

    let (ir, diagnostics) = lower_function(
        &engine,
        &map.functions,
        &CapturePlan::analyze(&map.functions),
        map.unit_ty.clone(),
        None,
    );
    assert!(diagnostics.is_empty());
    assert_eq!(ir.root().effect(), &effect);
    let perform = ir
        .root()
        .reachables()
        .expressions()
        .find_map(|id| match ir.root().get_expression(id).kind() {
            IRExprKind::Perform(perform) => Some(perform),
            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Call(_)
            | IRExprKind::Handle(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::Closure(_) => None,
        })
        .expect("effect operation should lower to perform");
    assert_eq!(perform.effect_id(), effect_id);
    assert_eq!(perform.operation_id(), operation_id);
    assert_eq!(perform.arguments().len(), 1);
}

#[tokio::test]
async fn run_with_lowers_body_and_handlers_to_explicit_handle_functions() {
    let engine = rayc_qbice::create_minimal_engine().await;
    let mut map = TestMap::new(&engine);
    let root = map.functions.root_id();
    let effect_id = TargetID::TEST.make_global(SymbolID::from_u128(10));
    let operation = SymbolID::from_u128(11);
    let operation_id = TargetID::TEST.make_global(operation);
    let effect_label =
        engine.intern(EffectLabel::new(effect_id, rayc_type::ty::args::Args::new([], &engine)));
    let body_effect = Ty::new_effect_row([effect_label], None, &engine);
    let body = map.functions.insert_thunk(map.unit_ty.clone(), body_effect.clone());
    let handler = map.functions.insert_operation_handler(
        operation_id,
        map.unit_ty.clone(),
        map.effect.clone(),
    );
    let parameter = map.operation_handler_parameter(handler, "value");
    let parameter_read = map.identifier(handler, parameter);
    map.statement(handler, parameter_read);
    let mut handlers = FxHashMap::default();
    handlers.insert(operation, handler);
    let run_with = map.expression(
        root,
        TypedExprKind::RunWith(RunWith::new(effect_id, Subst::new_empty(), body, handlers)),
        map.unit_ty.clone(),
    );
    map.statement(root, run_with);

    let (ir, diagnostics) = lower_function(
        &engine,
        &map.functions,
        &CapturePlan::analyze(&map.functions),
        map.unit_ty.clone(),
        None,
    );
    assert!(diagnostics.is_empty());
    let handle = ir
        .root()
        .reachables()
        .expressions()
        .find_map(|id| match ir.root().get_expression(id).kind() {
            IRExprKind::Handle(handle) => Some(handle),
            IRExprKind::Error
            | IRExprKind::Literal(_)
            | IRExprKind::RefOf(_)
            | IRExprKind::Load(_)
            | IRExprKind::Phi(_)
            | IRExprKind::Binary(_)
            | IRExprKind::Call(_)
            | IRExprKind::Perform(_)
            | IRExprKind::Tuple(_)
            | IRExprKind::Closure(_) => None,
        })
        .expect("run-with should lower to handle");
    assert_eq!(handle.effect_id(), effect_id);
    assert_eq!(handle.residual_effect(), &map.effect);
    assert_eq!(handle.handlers().len(), 1);
    assert_eq!(handle.handlers()[0].operation_id(), operation_id);

    let body = ir.get_function(handle.body().function_id());
    assert_eq!(body.effect(), &body_effect);
    assert_eq!(body.context().assert_as_thunk_context().return_ty(), &map.unit_ty);
    let handler = ir.get_function(handle.handlers()[0].function_id());
    let handler_context = handler.context().assert_as_operation_handler_context();
    assert_eq!(handler_context.operation(), operation_id);
    assert_eq!(handler_context.return_ty(), &map.unit_ty);
    let parameters = handler_context.parameters().collect::<Vec<_>>();
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0].1.ty(), &map.int_ty);
    let read = handler.reachables().expressions().next().expect("handler parameter should be read");
    let IRExprKind::Load(load) = handler.get_expression(read).kind() else {
        panic!("operation handler parameter read should lower to a load");
    };
    assert_eq!(load.address().root(), AddressRoot::OperationHandlerParameter(parameters[0].0));
}
