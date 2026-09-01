use rayc_ir::{
    cfg::Instruction as IRInstruction,
    ir_function::{FunctionID as IRFunctionID, IRContext, IRFunction},
};
use rayc_mono_ir::{
    MonoIR,
    function::{Local, LocalID, LocalKind, MonoFunctionID},
    instruction::{Assign, Instruction},
    operand::Operand,
    place::{FieldIndex, Place},
    rvalue::{Cast, Rvalue},
    ty::{MonoType, PointerMutability},
};

use super::{Builder, FunctionState};
use crate::function_abi::FunctionABI;

impl Builder {
    pub(super) async fn lower_function(
        &mut self,
        source_id: IRFunctionID,
        target_id: MonoFunctionID,
        output: &mut MonoIR,
    ) {
        let source = self.source.get_function(source_id).clone();
        let abi = self.function_abi(source_id).clone();
        let mut state =
            self.initialize_function_state(source_id, &source, &abi, target_id, output).await;

        for source_block in source.reachables().blocks() {
            let target_block = state.block(source_block);
            for instruction in source.block_instructions(source_block) {
                match instruction {
                    IRInstruction::Expression(expression_id) => {
                        self.lower_expression(
                            *expression_id,
                            target_block,
                            &source,
                            target_id,
                            &mut state,
                            output,
                        )
                        .await;
                    }
                    IRInstruction::Store(store) => {
                        let destination = Self::lower_address(store.address(), &state);
                        let value = Rvalue::Use(state.expression_operand(store.expression()));
                        output.push_instruction(
                            state.target_id,
                            target_block,
                            Instruction::Assign(Assign::new(destination, value)),
                        );
                    }
                }
            }
            let terminator = source
                .block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            Self::lower_terminator(source_block, target_block, terminator, &source, &state, output);
        }
    }

    async fn initialize_function_state(
        &mut self,
        source_id: IRFunctionID,
        source: &IRFunction,
        abi: &FunctionABI,
        target_id: MonoFunctionID,
        output: &mut MonoIR,
    ) -> FunctionState {
        let mut state = FunctionState::new(source_id, target_id);

        let environment_parameter = {
            let function = output.get_function(target_id);
            let mut parameters = function.parameters();
            let environment_parameter = match source.context() {
                IRContext::Def => {
                    for ((parameter_id, _), target_id) in
                        self.root_parameters.iter().zip(parameters.by_ref())
                    {
                        state.parameters.insert(parameter_id, target_id);
                    }
                    None
                }
                IRContext::Lambda(context) => {
                    let environment = parameters.next().expect("lambda should have an environment");
                    for ((parameter_id, _), target_id) in
                        context.parameters().zip(parameters.by_ref())
                    {
                        state.lambda_parameters.insert(parameter_id, target_id);
                    }
                    Some(environment)
                }
                IRContext::Thunk(_) => {
                    Some(parameters.next().expect("thunk should have an environment"))
                }
                IRContext::OperationHandler(context) => {
                    let environment =
                        parameters.next().expect("operation handler should have an environment");
                    for ((parameter_id, _), target_id) in
                        context.parameters().zip(parameters.by_ref())
                    {
                        state.operation_parameters.insert(parameter_id, target_id);
                    }
                    Some(environment)
                }
            };

            if !abi.captures_effect_handlers() {
                for (effect, parameter) in abi.effects().cloned().zip(parameters.by_ref()) {
                    state.handlers.insert(effect, Place::new(parameter));
                }
            }
            assert!(parameters.next().is_none(), "MonoIR signature parameters should be consumed");
            environment_parameter
        };

        for (variable_id, variable) in source.variables() {
            let ty = self.types.lower_type(variable.ty(), self.instance.substitution()).await;
            let local = output.insert_local(target_id, Local::new(ty, LocalKind::Variable));
            state.variables.insert(variable_id, local);
        }
        for expression_id in source.reachables().expressions() {
            let expression = source.get_expression(expression_id);
            let ty = self.types.lower_type(expression.ty(), self.instance.substitution()).await;
            let local = output.insert_local(target_id, Local::new(ty, LocalKind::Temporary));
            state.expressions.insert(expression_id, local);
        }

        for source_block in source.reachables().blocks() {
            let target_block = if source_block == source.entry_block() {
                output.entry_block(target_id)
            } else {
                output.create_block(target_id)
            };
            state.blocks.insert(source_block, target_block);
        }

        if let Some(environment_parameter) = environment_parameter {
            self.initialize_environment_access(
                source,
                abi,
                environment_parameter,
                target_id,
                &mut state,
                output,
            );
        }
        state
    }

    fn initialize_environment_access(
        &mut self,
        source: &IRFunction,
        abi: &FunctionABI,
        environment_parameter: LocalID,
        target_id: MonoFunctionID,
        state: &mut FunctionState,
        output: &mut MonoIR,
    ) {
        let environment_type = abi.environment_type();
        let MonoType::Aggregate(environment) = &*environment_type else {
            panic!("nested function environment should be an aggregate")
        };
        if environment.fields().is_empty() {
            return;
        }

        let pointer_type = self.types.pointer(environment_type, PointerMutability::Const);
        let pointer_local =
            output.insert_local(target_id, Local::new(pointer_type.clone(), LocalKind::Temporary));
        let entry = output.entry_block(target_id);
        output.push_instruction(
            target_id,
            entry,
            Instruction::Assign(Assign::new(
                Place::new(pointer_local),
                Rvalue::Cast(Cast::new(
                    Operand::Copy(Place::new(environment_parameter)),
                    pointer_type,
                )),
            )),
        );
        let environment_place = Place::new(pointer_local).dereference();
        for (index, capture_id) in abi.capture_ids().enumerate() {
            state.captures.insert(
                capture_id,
                environment_place.clone().project_field(FieldIndex::new(index.try_into().unwrap())),
            );
        }
        if abi.captures_effect_handlers() {
            for (offset, effect) in abi.effects().cloned().enumerate() {
                let index = abi.capture_count() + offset;
                state.handlers.insert(
                    effect,
                    environment_place
                        .clone()
                        .project_field(FieldIndex::new(index.try_into().unwrap())),
                );
            }
        }

        let expected_captures = match source.context() {
            IRContext::Lambda(context) => context.captures().len(),
            IRContext::Thunk(context) => context.captures().len(),
            IRContext::OperationHandler(context) => context.captures().len(),
            IRContext::Def => 0,
        };
        assert_eq!(expected_captures, abi.capture_count());
    }
}
