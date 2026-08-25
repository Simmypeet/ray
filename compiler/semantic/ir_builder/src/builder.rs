use std::mem;

use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    expression::{Expression, ExpressionID, ExpressionKind, load::Load},
    function::{
        Context as IrContext, Function as IrFunction, FunctionID as IrFunctionID, FunctionMap,
    },
    lambda::{Capture, CaptureID, LambdaParameter as IrLambdaParameter, LambdaParameterID},
    variable::{Variable, VariableID},
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::{Mutability, Ty};
use rayc_typed_ast::{
    lambda::LambdaParameterID as TypedLambdaParameterID,
    name_binding::Source,
    typed_function::{Context as TypedContext, FunctionID as TypedFunctionID},
    variable::VariableID as TypedVariableID,
};

use crate::context::LoweringContext;

struct FunctionBuildState {
    function: IrFunction,
    current_block: BlockID,
    typed_function_id: TypedFunctionID,
    variables: FxHashMap<TypedVariableID, VariableID>,
    lambda_parameters: FxHashMap<TypedLambdaParameterID, LambdaParameterID>,
    captures: FxHashMap<Source, CaptureID>,
}

impl FunctionBuildState {
    fn new(context: &LoweringContext<'_>) -> Self {
        let typed_function_id = context.typed_function_id();
        let capture_plan = context.capture_plan(typed_function_id);
        let mut lambda_parameters = FxHashMap::default();
        let mut captures = FxHashMap::default();
        let function = match context.typed_function_context() {
            TypedContext::Def(_) => {
                assert_eq!(
                    typed_function_id,
                    context.root_typed_function_id(),
                    "only the root TypedAST function should be a def"
                );
                assert_eq!(
                    capture_plan.captures().len(),
                    0,
                    "root TypedAST function should not capture a source"
                );
                IrFunction::new()
            }
            TypedContext::Lambda(lambda_context) => {
                assert_ne!(
                    typed_function_id,
                    context.root_typed_function_id(),
                    "root TypedAST function should not be a lambda"
                );
                let mut function = IrFunction::new_lambda();
                for (typed_id, parameter) in lambda_context.parameters() {
                    let ir_id = function.insert_lambda_parameter(IrLambdaParameter::new(
                        parameter.ty().clone(),
                        parameter.span(),
                    ));
                    assert!(lambda_parameters.insert(typed_id, ir_id).is_none());
                }
                for (_, requirement) in capture_plan.captures() {
                    let ir_id = function.insert_capture(Capture::new(
                        requirement.pointee_ty().clone(),
                        requirement.mutability(),
                        requirement.span(),
                    ));
                    assert!(captures.insert(requirement.source(), ir_id).is_none());
                }
                function
            }
        };
        let current_block = function.entry_block();
        Self {
            function,
            current_block,
            typed_function_id,
            variables: FxHashMap::default(),
            lambda_parameters,
            captures,
        }
    }
}

pub struct Builder {
    engine: TrackedEngine,
    ir_functions: FunctionMap,
    building_function: FunctionBuildState,
    suspended_functions: Vec<FunctionBuildState>,
}

impl Builder {
    pub fn new(engine: TrackedEngine, context: &LoweringContext<'_>) -> Self {
        let building_function = FunctionBuildState::new(context);
        let ir_functions = FunctionMap::new(IrFunction::new());

        Self { engine, ir_functions, building_function, suspended_functions: Vec::new() }
    }

    pub fn lower(mut self, context: &LoweringContext<'_>) -> FunctionMap {
        self.lower_current_function(context);
        assert!(
            self.suspended_functions.is_empty(),
            "all suspended IR functions should be restored before finishing lowering"
        );
        assert_eq!(
            self.building_function.typed_function_id,
            context.root_typed_function_id(),
            "root IR function should be active when lowering finishes"
        );
        self.ir_functions.replace_root(self.building_function.function);
        self.ir_functions
    }

    pub fn lower_lambda_function(
        &mut self,
        context: &LoweringContext<'_>,
        typed_function_id: TypedFunctionID,
    ) -> IrFunctionID {
        let lambda_context = context.for_function(typed_function_id);
        self.start_lambda(&lambda_context);
        self.lower_current_function(&lambda_context);
        self.finish_lambda()
    }

    fn lower_current_function(&mut self, context: &LoweringContext<'_>) {
        self.lower_statements(context);
    }

    fn start_lambda(&mut self, context: &LoweringContext<'_>) {
        let lambda = FunctionBuildState::new(context);
        let enclosing = mem::replace(&mut self.building_function, lambda);
        self.suspended_functions.push(enclosing);
    }

    fn finish_lambda(&mut self) -> IrFunctionID {
        let enclosing = self
            .suspended_functions
            .pop()
            .expect("a lambda should suspend its enclosing IR function");
        let lambda = mem::replace(&mut self.building_function, enclosing);
        self.ir_functions.insert_lambda(lambda.function)
    }

    pub fn emit_expression(&mut self, expression: Expression) -> ExpressionID {
        let expression_id = self.building_function.function.insert_expression(expression);
        self.building_function
            .function
            .push_expression(self.building_function.current_block, expression_id);
        expression_id
    }

    pub fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.building_function.function.push_store(
            self.building_function.current_block,
            address,
            value,
        );
    }

    pub fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> VariableID {
        self.building_function.function.insert_variable(Variable::new(ty, span))
    }

    pub fn register_source_variable(
        &mut self,
        context: &LoweringContext<'_>,
        typed_id: TypedVariableID,
    ) -> VariableID {
        let variable = context.variable(typed_id);
        let ir_id = self
            .building_function
            .function
            .insert_variable(Variable::new(variable.ty().clone(), variable.span()));
        self.building_function.variables.insert(typed_id, ir_id);
        ir_id
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.building_function
            .function
            .set_terminator(self.building_function.current_block, terminator);
    }

    pub fn jump_to(&mut self, target: BlockID) -> BlockID {
        let predecessor = self.building_function.current_block;
        self.terminate(Terminator::Jump(target));
        predecessor
    }

    pub fn is_terminated(&self) -> bool {
        self.building_function
            .function
            .block_terminator(self.building_function.current_block)
            .is_some()
    }

    pub fn create_block(&mut self) -> BlockID { self.building_function.function.create_block() }

    pub const fn select_block(&mut self, block: BlockID) {
        self.building_function.current_block = block;
    }

    pub fn source_variable(&self, id: TypedVariableID) -> Option<VariableID> {
        self.building_function.variables.get(&id).copied()
    }

    pub fn source_address(&mut self, source: Source) -> Address {
        if source.function_id() == self.building_function.typed_function_id {
            return match source {
                Source::Variable(id) => {
                    let variable_id = self
                        .source_variable(id.local_id())
                        .expect("local source variable should be registered before use");
                    self.variable_address(variable_id)
                }
                Source::Parameter(id) => self.parameter_address(id.local_id()),
                Source::LambdaParameter(id) => {
                    let parameter_id = self
                        .building_function
                        .lambda_parameters
                        .get(&id.local_id())
                        .copied()
                        .expect("lambda parameter should be registered before use");
                    self.lambda_parameter_address(parameter_id)
                }
            };
        }

        let capture_id = self
            .building_function
            .captures
            .get(&source)
            .copied()
            .expect("non-local source should have an analyzed capture");
        let (span, pointer_ty) = match self.building_function.function.context() {
            IrContext::Def => panic!("def functions should not contain captures"),
            IrContext::Lambda(lambda_context) => {
                let capture = lambda_context.get_capture(capture_id);
                (
                    capture.span(),
                    self.pointer_ty(capture.pointee_ty().clone(), capture.mutability()),
                )
            }
        };
        let pointer = self.emit_expression(Expression::new(
            ExpressionKind::Load(Load::new(self.capture_address(capture_id))),
            span,
            pointer_ty,
        ));
        self.dereference_address(pointer)
    }

    pub fn pointer_ty(&self, pointee_ty: Interned<Ty>, mutability: Mutability) -> Interned<Ty> {
        Ty::new_pointer(pointee_ty, mutability, &self.engine)
    }

    pub fn error_address(&self) -> Address { Address::new_error(&self.engine) }

    pub fn variable_address(&self, id: VariableID) -> Address {
        Address::new_variable(id, &self.engine)
    }

    pub fn parameter_address(&self, id: ParameterID) -> Address {
        Address::new_parameter(id, &self.engine)
    }

    pub fn lambda_parameter_address(&self, id: LambdaParameterID) -> Address {
        Address::new_lambda_parameter(id, &self.engine)
    }

    pub fn capture_address(&self, id: CaptureID) -> Address {
        Address::new_capture(id, &self.engine)
    }

    pub fn dereference_address(&self, value: ExpressionID) -> Address {
        Address::new_deref(value, &self.engine)
    }

    pub fn project_tuple(&self, address: &mut Address, index: usize) {
        address.add_tuple_index(index, &self.engine);
    }
}
