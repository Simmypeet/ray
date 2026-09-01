use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::BlockID as IRBlockID,
    ir_expr::IRExprID,
    ir_function::FunctionID as IRFunctionID,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    MonoEffectInstance, MonoIR,
    cfg::{BlockID, Terminator},
    function::{Local, LocalID, MonoFunctionID},
    instruction::Instruction,
    operand::Operand,
    place::Place,
    ty::MonoType,
};
use rayc_semantic_element::parameter::ParameterID;

/// Mutable output and identity mappings for lowering one function.
pub(super) struct BuilderState<'output> {
    output: &'output mut MonoIR,
    state: FunctionState,
}

/// Maps semantic IR identities to the locals and blocks of one target function.
struct FunctionState {
    source_id: IRFunctionID,
    target_id: MonoFunctionID,
    blocks: FxHashMap<IRBlockID, BlockID>,
    expressions: FxHashMap<IRExprID, LocalID>,
    variables: FxHashMap<IRVariableID, LocalID>,
    parameters: FxHashMap<ParameterID, LocalID>,
    lambda_parameters: FxHashMap<LambdaParameterID, LocalID>,
    operation_parameters: FxHashMap<OperationHandlerParameterID, LocalID>,
    captures: FxHashMap<CaptureID, Place>,
    handlers: FxHashMap<MonoEffectInstance, Place>,
}

impl<'output> BuilderState<'output> {
    pub(super) fn new(
        output: &'output mut MonoIR,
        source_id: IRFunctionID,
        target_id: MonoFunctionID,
    ) -> Self {
        Self {
            output,
            state: FunctionState {
                source_id,
                target_id,
                blocks: FxHashMap::default(),
                expressions: FxHashMap::default(),
                variables: FxHashMap::default(),
                parameters: FxHashMap::default(),
                lambda_parameters: FxHashMap::default(),
                operation_parameters: FxHashMap::default(),
                captures: FxHashMap::default(),
                handlers: FxHashMap::default(),
            },
        }
    }

    pub(super) const fn target_id(&self) -> MonoFunctionID { self.state.target_id }

    pub(super) fn parameter_ids(&self) -> Vec<LocalID> {
        self.output.get_function(self.target_id()).parameters().collect()
    }

    pub(super) fn local_type(&self, local: LocalID) -> qbice::storage::intern::Interned<MonoType> {
        self.output.get_function(self.target_id()).get_local(local).ty().clone()
    }

    pub(super) fn insert_local(&mut self, local: Local) -> LocalID {
        self.output.insert_local(self.target_id(), local)
    }

    pub(super) fn entry_block(&self) -> BlockID { self.output.entry_block(self.target_id()) }

    pub(super) fn create_block(&mut self) -> BlockID { self.output.create_block(self.target_id()) }

    pub(super) fn push_instruction(&mut self, block: BlockID, instruction: Instruction) {
        self.output.push_instruction(self.target_id(), block, instruction);
    }

    pub(super) fn set_terminator(&mut self, block: BlockID, terminator: Terminator) {
        self.output.set_terminator(self.target_id(), block, terminator);
    }

    pub(super) fn insert_block(&mut self, source: IRBlockID, target: BlockID) {
        self.state.blocks.insert(source, target);
    }

    pub(super) fn block(&self, block: IRBlockID) -> BlockID {
        *self.state.blocks.get(&block).expect("semantic IR block should be mapped")
    }

    pub(super) fn insert_expression(&mut self, expression: IRExprID, local: LocalID) {
        self.state.expressions.insert(expression, local);
    }

    pub(super) fn expression_place(&self, expression: IRExprID) -> Place {
        Place::new(
            *self
                .state
                .expressions
                .get(&expression)
                .expect("semantic IR expression should be mapped"),
        )
    }

    pub(super) fn expression_operand(&self, expression: IRExprID) -> Operand {
        Operand::Copy(self.expression_place(expression))
    }

    pub(super) fn insert_variable(&mut self, variable: IRVariableID, local: LocalID) {
        self.state.variables.insert(variable, local);
    }

    pub(super) fn variable_place(&self, variable: IRVariableID) -> Place {
        Place::new(
            *self.state.variables.get(&variable).expect("semantic IR variable should be mapped"),
        )
    }

    pub(super) fn insert_parameter(&mut self, parameter: ParameterID, local: LocalID) {
        self.state.parameters.insert(parameter, local);
    }

    pub(super) fn parameter_place(&self, parameter: ParameterID) -> Place {
        Place::new(
            *self.state.parameters.get(&parameter).expect("semantic parameter should be mapped"),
        )
    }

    pub(super) fn insert_lambda_parameter(&mut self, parameter: LambdaParameterID, local: LocalID) {
        self.state.lambda_parameters.insert(parameter, local);
    }

    pub(super) fn lambda_parameter_place(&self, parameter: LambdaParameterID) -> Place {
        Place::new(
            *self
                .state
                .lambda_parameters
                .get(&parameter)
                .expect("semantic lambda parameter should be mapped"),
        )
    }

    pub(super) fn insert_operation_parameter(
        &mut self,
        parameter: OperationHandlerParameterID,
        local: LocalID,
    ) {
        self.state.operation_parameters.insert(parameter, local);
    }

    pub(super) fn operation_parameter_place(
        &self,
        parameter: OperationHandlerParameterID,
    ) -> Place {
        Place::new(
            *self
                .state
                .operation_parameters
                .get(&parameter)
                .expect("semantic operation parameter should be mapped"),
        )
    }

    pub(super) fn insert_capture(&mut self, capture: CaptureID, place: Place) {
        self.state.captures.insert(capture, place);
    }

    pub(super) fn capture_place(&self, capture: CaptureID) -> Place {
        self.state.captures.get(&capture).expect("semantic capture should be mapped").clone()
    }

    pub(super) fn insert_handler(&mut self, effect: MonoEffectInstance, place: Place) {
        self.state.handlers.insert(effect, place);
    }

    pub(super) fn handler_place(&self, effect: &MonoEffectInstance) -> Place {
        self.state
            .handlers
            .get(effect)
            .unwrap_or_else(|| {
                panic!("effect handler {effect:?} is not available in {:#?}", self.state.source_id)
            })
            .clone()
    }

    pub(super) fn handler_operand(&self, effect: &MonoEffectInstance) -> Operand {
        Operand::Copy(self.handler_place(effect))
    }
}
