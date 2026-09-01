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
    ty::PointerMutability,
};

use crate::{builder::Builder, context::Context, function_abi::FunctionABI};

impl Builder<'_> {
    pub(crate) async fn lower_function(&mut self, context: &Context, source_id: IRFunctionID) {
        let source = context.source_function(source_id);
        let abi = context.function_abi(source_id);
        self.initialize_function_state(context, &source, abi).await;

        for source_block in source.reachables().blocks() {
            let target_block = self.block(source_block);
            for instruction in source.block_instructions(source_block) {
                match instruction {
                    IRInstruction::Expression(expression_id) => {
                        self.lower_expression(context, *expression_id, target_block, &source).await;
                    }
                    IRInstruction::Store(store) => {
                        let destination = self.lower_address(store.address());
                        let value = Rvalue::Use(self.expression_operand(store.expression()));
                        self.push_instruction(
                            target_block,
                            Instruction::Assign(Assign::new(destination, value)),
                        );
                    }
                }
            }
            let terminator = source
                .block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            self.lower_terminator(source_block, target_block, terminator, &source);
        }
    }

    async fn initialize_function_state(
        &mut self,
        context: &Context,
        source: &IRFunction,
        abi: &FunctionABI,
    ) {
        let environment_parameter = {
            let mut parameters = self.parameter_ids().into_iter();
            let environment_parameter = match source.context() {
                IRContext::Def => {
                    for ((parameter_id, _), target_id) in
                        context.get_root_parameter_map().await.iter().zip(parameters.by_ref())
                    {
                        self.insert_parameter(parameter_id, target_id);
                    }
                    None
                }
                IRContext::Lambda(context) => {
                    let environment = parameters.next().expect("lambda should have an environment");
                    for ((parameter_id, _), target_id) in
                        context.parameters().zip(parameters.by_ref())
                    {
                        self.insert_lambda_parameter(parameter_id, target_id);
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
                        self.insert_operation_parameter(parameter_id, target_id);
                    }
                    Some(environment)
                }
            };

            if !abi.captures_effect_handlers() {
                for (effect, parameter) in abi.effects().cloned().zip(parameters.by_ref()) {
                    self.insert_handler(effect, Place::new(parameter));
                }
            }
            assert!(parameters.next().is_none(), "MonoIR signature parameters should be consumed");
            environment_parameter
        };

        for (variable_id, variable) in source.variables() {
            let ty = context.lower_type(variable.ty()).await;
            let local = self.insert_local(Local::new(ty, LocalKind::Variable));
            self.insert_variable(variable_id, local);
        }
        for expression_id in source.reachables().expressions() {
            let expression = source.get_expression(expression_id);
            let ty = context.lower_type(expression.ty()).await;
            let local = self.insert_local(Local::new(ty, LocalKind::Temporary));
            self.insert_expression(expression_id, local);
        }

        for source_block in source.reachables().blocks() {
            let target_block = if source_block == source.entry_block() {
                self.entry_block()
            } else {
                self.create_block()
            };
            self.insert_block(source_block, target_block);
        }

        if let Some(environment_parameter) = environment_parameter {
            self.initialize_environment_access(context, source, abi, environment_parameter);
        }
    }

    fn initialize_environment_access(
        &mut self,
        context: &Context,
        source: &IRFunction,
        abi: &FunctionABI,
        environment_parameter: LocalID,
    ) {
        let env = abi.environment_type();
        if env.captures().is_empty() {
            return;
        }

        let env_ty = context.intern_environment_type(env.clone());
        let pointer_type = context.create_pointer(env_ty, PointerMutability::Mut);
        let pointer_local =
            self.insert_local(Local::new(pointer_type.clone(), LocalKind::Temporary));

        let entry = self.entry_block();

        // Case from the `void* env` parameter to the `Environment* env` local
        self.push_instruction(
            entry,
            Instruction::Assign(Assign::new(
                Place::new(pointer_local),
                Rvalue::Cast(Cast::new(
                    Operand::Copy(Place::new(environment_parameter)),
                    pointer_type,
                )),
            )),
        );

        // in order to access the environment fields, we need to dereference the pointer
        // local esentially, we create a `env->field` or `(*env).field` place
        // for each capture and handler in the environment
        let environment_place = Place::new(pointer_local).dereference();
        for (index, capture_id) in abi.capture_ids().enumerate() {
            self.insert_capture(
                capture_id,
                environment_place
                    .clone()
                    .project_environment_field(FieldIndex::new(index.try_into().unwrap())),
            );
        }

        // if the environment explicitly captures effect handlers in the environment, we
        // need to insert them as well
        if abi.captures_effect_handlers() {
            for (offset, effect) in abi.effects().cloned().enumerate() {
                let index = abi.capture_count() + offset;
                self.insert_handler(
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
