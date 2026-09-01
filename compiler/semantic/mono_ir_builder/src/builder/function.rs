use rayc_ir::{
    cfg::Instruction as IRInstruction,
    ir_function::{FunctionID as IRFunctionID, IRContext, IRFunction},
};
use rayc_mono_ir::{
    function::{Local, LocalID, LocalKind},
    instruction::{Assign, Instruction},
    operand::Operand,
    place::{FieldIndex, Place},
    rvalue::{Cast, Rvalue},
    ty::{AggregateType, MonoType, PointerMutability},
};

use super::Builder;
use crate::{context::Context, function_abi::FunctionABI};

impl Context {
    pub(crate) async fn lower_function(&self, source_id: IRFunctionID, builder: &mut Builder<'_>) {
        let source = self.source_function(source_id);
        let abi = self.function_abi(source_id);
        self.initialize_function_state(&source, &abi, builder).await;

        for source_block in source.reachables().blocks() {
            let target_block = builder.block(source_block);
            for instruction in source.block_instructions(source_block) {
                match instruction {
                    IRInstruction::Expression(expression_id) => {
                        self.lower_expression(*expression_id, target_block, &source, builder).await;
                    }
                    IRInstruction::Store(store) => {
                        let destination = Self::lower_address(store.address(), builder);
                        let value = Rvalue::Use(builder.expression_operand(store.expression()));
                        builder.push_instruction(
                            target_block,
                            Instruction::Assign(Assign::new(destination, value)),
                        );
                    }
                }
            }
            let terminator = source
                .block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            Self::lower_terminator(source_block, target_block, terminator, &source, builder);
        }
    }

    async fn initialize_function_state(
        &self,
        source: &IRFunction,
        abi: &FunctionABI,
        builder: &mut Builder<'_>,
    ) {
        let environment_parameter = {
            let mut parameters = builder.parameter_ids().into_iter();
            let environment_parameter = match source.context() {
                IRContext::Def => {
                    for ((parameter_id, _), target_id) in
                        self.get_root_parameter_map().await.iter().zip(parameters.by_ref())
                    {
                        builder.insert_parameter(parameter_id, target_id);
                    }
                    None
                }
                IRContext::Lambda(context) => {
                    let environment = parameters.next().expect("lambda should have an environment");
                    for ((parameter_id, _), target_id) in
                        context.parameters().zip(parameters.by_ref())
                    {
                        builder.insert_lambda_parameter(parameter_id, target_id);
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
                        builder.insert_operation_parameter(parameter_id, target_id);
                    }
                    Some(environment)
                }
            };

            if !abi.captures_effect_handlers() {
                for (effect, parameter) in abi.effects().cloned().zip(parameters.by_ref()) {
                    builder.insert_handler(effect, Place::new(parameter));
                }
            }
            assert!(parameters.next().is_none(), "MonoIR signature parameters should be consumed");
            environment_parameter
        };

        for (variable_id, variable) in source.variables() {
            let ty = self.lower_type(variable.ty()).await;
            let local = builder.insert_local(Local::new(ty, LocalKind::Variable));
            builder.insert_variable(variable_id, local);
        }
        for expression_id in source.reachables().expressions() {
            let expression = source.get_expression(expression_id);
            let ty = self.lower_type(expression.ty()).await;
            let local = builder.insert_local(Local::new(ty, LocalKind::Temporary));
            builder.insert_expression(expression_id, local);
        }

        for source_block in source.reachables().blocks() {
            let target_block = if source_block == source.entry_block() {
                builder.entry_block()
            } else {
                builder.create_block()
            };
            builder.insert_block(source_block, target_block);
        }

        if let Some(environment_parameter) = environment_parameter {
            self.initialize_environment_access(source, abi, environment_parameter, builder);
        }
    }

    fn initialize_environment_access(
        &self,
        source: &IRFunction,
        abi: &FunctionABI,
        environment_parameter: LocalID,
        builder: &mut Builder<'_>,
    ) {
        let environment_type = abi.environment_type();
        let MonoType::Aggregate(AggregateType::Environment(environment)) = &*environment_type
        else {
            panic!("nested function environment should be an aggregate")
        };
        if environment.captures().is_empty() {
            return;
        }

        let pointer_type = self.create_pointer(environment_type, PointerMutability::Const);
        let pointer_local =
            builder.insert_local(Local::new(pointer_type.clone(), LocalKind::Temporary));
        let entry = builder.entry_block();
        builder.push_instruction(
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
            builder.insert_capture(
                capture_id,
                environment_place
                    .clone()
                    .project_environment_field(FieldIndex::new(index.try_into().unwrap())),
            );
        }
        if abi.captures_effect_handlers() {
            for (offset, effect) in abi.effects().cloned().enumerate() {
                let index = abi.capture_count() + offset;
                builder.insert_handler(
                    effect,
                    environment_place
                        .clone()
                        .project_environment_field(FieldIndex::new(index.try_into().unwrap())),
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
