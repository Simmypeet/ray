use qbice::storage::intern::Interned;
use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::BlockID as IRBlockID,
    ir_expr::IRExprID,
    ir_lambda::{CaptureID, LambdaParameterID},
    ir_operation_handler::OperationHandlerParameterID,
    ir_variable::IRVariableID,
};
use rayc_mono_ir::{
    MonoEffectInstance, MonoIR,
    cfg::{BlockID, Terminator},
    function::{Local, LocalID, MonoFunctionID},
    instruction::{Call, Instruction},
    operand::Operand,
    place::Place,
    ty::MonoType,
};
use rayc_semantic_element::parameter::ParameterID;

/// Mutable output and identity mappings for lowering one function.
pub(crate) struct Builder<'output> {
    output: &'output mut MonoIR,
    state: FunctionState,
    block: BlockID,
}

/// Maps semantic IR identities to the locals and blocks of one target function.
struct FunctionState {
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

impl<'output> Builder<'output> {
    pub(crate) fn new(output: &'output mut MonoIR, target_id: MonoFunctionID) -> Self {
        let block = output.entry_block(target_id);
        Self {
            output,
            state: FunctionState {
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
            block,
        }
    }

    pub(crate) const fn target_id(&self) -> MonoFunctionID { self.state.target_id }

    pub(crate) fn parameter_ids(&self) -> Vec<LocalID> {
        self.output.get_function(self.target_id()).parameters().collect()
    }

    pub(crate) fn local_type(&self, local: LocalID) -> &Interned<MonoType> {
        self.output.get_function(self.target_id()).get_local(local).ty()
    }

    pub(crate) fn insert_local(&mut self, local: Local) -> LocalID {
        self.output.insert_local(self.target_id(), local)
    }

    pub(crate) fn entry_block(&self) -> BlockID { self.output.entry_block(self.target_id()) }

    pub(crate) fn create_block(&mut self) -> BlockID { self.output.create_block(self.target_id()) }

    pub(crate) const fn select_block(&mut self, block: BlockID) { self.block = block; }

    pub(crate) fn push_instruction(&mut self, instruction: Instruction) {
        self.output.push_instruction(self.target_id(), self.block, instruction);
    }

    pub(crate) fn push_call_with_destination(
        &mut self,
        destination: Place,
        callee: Operand,
        arguments: Vec<Operand>,
    ) {
        self.output.push_instruction(
            self.target_id(),
            self.block,
            Instruction::Call(Call::new(Some(destination), callee, arguments)),
        );
    }

    pub(crate) fn set_terminator(&mut self, terminator: Terminator) {
        self.output.set_terminator(self.target_id(), self.block, terminator);
    }

    pub(crate) fn insert_block(&mut self, source: IRBlockID, target: BlockID) {
        self.state.blocks.insert(source, target);
    }

    pub(crate) fn block(&self, block: IRBlockID) -> BlockID {
        *self.state.blocks.get(&block).expect("semantic IR block should be mapped")
    }

    pub(crate) fn insert_expression(&mut self, expression: IRExprID, local: LocalID) {
        self.state.expressions.insert(expression, local);
    }

    pub(crate) fn expression_place(&self, expression: IRExprID) -> Place {
        Place::new(
            *self
                .state
                .expressions
                .get(&expression)
                .expect("semantic IR expression should be mapped"),
        )
    }

    pub(crate) fn expression_operand(&self, expression: IRExprID) -> Operand {
        Operand::Copy(self.expression_place(expression))
    }

    pub(crate) fn insert_variable(&mut self, variable: IRVariableID, local: LocalID) {
        self.state.variables.insert(variable, local);
    }

    pub(crate) fn variable_place(&self, variable: IRVariableID) -> Place {
        Place::new(
            *self.state.variables.get(&variable).expect("semantic IR variable should be mapped"),
        )
    }

    pub(crate) fn insert_parameter(&mut self, parameter: ParameterID, local: LocalID) {
        self.state.parameters.insert(parameter, local);
    }

    pub(crate) fn parameter_place(&self, parameter: ParameterID) -> Place {
        Place::new(
            *self.state.parameters.get(&parameter).expect("semantic parameter should be mapped"),
        )
    }

    pub(crate) fn insert_lambda_parameter(&mut self, parameter: LambdaParameterID, local: LocalID) {
        self.state.lambda_parameters.insert(parameter, local);
    }

    pub(crate) fn lambda_parameter_place(&self, parameter: LambdaParameterID) -> Place {
        Place::new(
            *self
                .state
                .lambda_parameters
                .get(&parameter)
                .expect("semantic lambda parameter should be mapped"),
        )
    }

    pub(crate) fn insert_operation_parameter(
        &mut self,
        parameter: OperationHandlerParameterID,
        local: LocalID,
    ) {
        self.state.operation_parameters.insert(parameter, local);
    }

    pub(crate) fn operation_parameter_place(
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

    pub(crate) fn insert_capture(&mut self, capture: CaptureID, place: Place) {
        self.state.captures.insert(capture, place);
    }

    pub(crate) fn capture_place(&self, capture: CaptureID) -> Place {
        self.state.captures.get(&capture).expect("semantic capture should be mapped").clone()
    }

    pub(crate) fn insert_handler(&mut self, effect: MonoEffectInstance, place: Place) {
        self.state.handlers.insert(effect, place);
    }

    pub(crate) fn handler_place(&self, effect: &MonoEffectInstance) -> Place {
        self.state
            .handlers
            .get(effect)
            .unwrap_or_else(|| {
                panic!("effect handler {effect:?} is not available in {:?}", self.state.target_id)
            })
            .clone()
    }

    pub(crate) fn handler_operand(&self, effect: &MonoEffectInstance) -> Operand {
        Operand::Copy(self.handler_place(effect))
    }
}
