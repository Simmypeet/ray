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
    MonoEffectInstance,
    cfg::BlockID,
    function::{LocalID, MonoFunctionID},
    operand::Operand,
    place::Place,
};
use rayc_semantic_element::parameter::ParameterID;

/// Maps semantic IR identities to the locals and blocks of one target function.
pub(super) struct FunctionState {
    pub(super) source_id: IRFunctionID,
    pub(super) target_id: MonoFunctionID,
    pub(super) blocks: FxHashMap<IRBlockID, BlockID>,
    pub(super) expressions: FxHashMap<IRExprID, LocalID>,
    pub(super) variables: FxHashMap<IRVariableID, LocalID>,
    pub(super) parameters: FxHashMap<ParameterID, LocalID>,
    pub(super) lambda_parameters: FxHashMap<LambdaParameterID, LocalID>,
    pub(super) operation_parameters: FxHashMap<OperationHandlerParameterID, LocalID>,
    pub(super) captures: FxHashMap<CaptureID, Place>,
    pub(super) handlers: FxHashMap<MonoEffectInstance, Place>,
}

impl FunctionState {
    pub(super) fn new(source_id: IRFunctionID, target_id: MonoFunctionID) -> Self {
        Self {
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
        }
    }

    pub(super) fn block(&self, block: IRBlockID) -> BlockID {
        *self.blocks.get(&block).expect("semantic IR block should be mapped")
    }

    pub(super) fn expression_place(&self, expression: IRExprID) -> Place {
        Place::new(
            *self.expressions.get(&expression).expect("semantic IR expression should be mapped"),
        )
    }

    pub(super) fn expression_operand(&self, expression: IRExprID) -> Operand {
        Operand::Copy(self.expression_place(expression))
    }

    pub(super) fn variable_place(&self, variable: IRVariableID) -> Place {
        Place::new(*self.variables.get(&variable).expect("semantic IR variable should be mapped"))
    }

    pub(super) fn parameter_place(&self, parameter: ParameterID) -> Place {
        Place::new(*self.parameters.get(&parameter).expect("semantic parameter should be mapped"))
    }

    pub(super) fn lambda_parameter_place(&self, parameter: LambdaParameterID) -> Place {
        Place::new(
            *self
                .lambda_parameters
                .get(&parameter)
                .expect("semantic lambda parameter should be mapped"),
        )
    }

    pub(super) fn operation_parameter_place(
        &self,
        parameter: OperationHandlerParameterID,
    ) -> Place {
        Place::new(
            *self
                .operation_parameters
                .get(&parameter)
                .expect("semantic operation parameter should be mapped"),
        )
    }

    pub(super) fn capture_place(&self, capture: CaptureID) -> Place {
        self.captures.get(&capture).expect("semantic capture should be mapped").clone()
    }

    pub(super) fn handler_place(&self, effect: &MonoEffectInstance) -> Place {
        self.handlers
            .get(effect)
            .unwrap_or_else(|| {
                panic!("effect handler {effect:?} is not available in {:#?}", self.source_id)
            })
            .clone()
    }

    pub(super) fn handler_operand(&self, effect: &MonoEffectInstance) -> Operand {
        Operand::Copy(self.handler_place(effect))
    }
}
