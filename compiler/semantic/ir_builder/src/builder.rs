use std::mem;

use qbice::storage::intern::Interned;
use rayc_ir::{
    address::Address,
    cfg::{BlockID, Terminator},
    expression::{Expression, ExpressionID, ExpressionKind, load::Load},
    function::{Function as IrFunction, FunctionID as IrFunctionID, FunctionMap},
    lambda::{CaptureID, LambdaParameterID},
    variable::VariableID,
};
use rayc_lexical::tree::RelativeSpan;
use rayc_qbice::TrackedEngine;
use rayc_semantic_element::parameter::ParameterID;
use rayc_type::ty::{Mutability, Ty};
use rayc_typed_ast::{
    name_binding::Source, typed_function::FunctionID as TypedFunctionID,
    variable::VariableID as TypedVariableID,
};

use crate::{
    context::LoweringContext,
    function_build_state::{FunctionBuildState, SourceLocation},
};

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
            self.building_function.typed_function_id(),
            context.root_typed_function_id(),
            "root IR function should be active when lowering finishes"
        );
        self.ir_functions.replace_root(self.building_function.into_function());
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
        self.ir_functions.insert_lambda(lambda.into_function())
    }

    pub fn emit_expression(&mut self, expression: Expression) -> ExpressionID {
        self.building_function.emit_expression(expression)
    }

    pub fn emit_store(&mut self, address: Address, value: ExpressionID) {
        self.building_function.emit_store(address, value);
    }

    pub fn create_temporary(&mut self, ty: Interned<Ty>, span: RelativeSpan) -> VariableID {
        self.building_function.create_temporary(ty, span)
    }

    pub fn register_source_variable(
        &mut self,
        context: &LoweringContext<'_>,
        typed_id: TypedVariableID,
    ) -> VariableID {
        self.building_function.register_source_variable(context, typed_id)
    }

    pub fn terminate(&mut self, terminator: Terminator) {
        self.building_function.terminate(terminator);
    }

    pub fn jump_to(&mut self, target: BlockID) -> BlockID { self.building_function.jump_to(target) }

    pub fn is_terminated(&self) -> bool { self.building_function.is_terminated() }

    pub fn create_block(&mut self) -> BlockID { self.building_function.create_block() }

    pub const fn select_block(&mut self, block: BlockID) {
        self.building_function.select_block(block);
    }

    pub fn source_address(&mut self, source: Source) -> Address {
        match self.building_function.resolve_source(source) {
            SourceLocation::Variable(id) => self.variable_address(id),
            SourceLocation::Parameter(id) => self.parameter_address(id),
            SourceLocation::LambdaParameter(id) => self.lambda_parameter_address(id),
            SourceLocation::Capture { id, span, pointee_ty, mutability } => {
                let pointer_ty = self.pointer_ty(pointee_ty, mutability);
                let pointer = self.emit_expression(Expression::new(
                    ExpressionKind::Load(Load::new(self.capture_address(id))),
                    span,
                    pointer_ty,
                ));
                self.dereference_address(pointer)
            }
        }
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
