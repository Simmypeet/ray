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
        self.initialize_function_state(context, source_id, &source, abi).await;

        for source_block in source.reachables().blocks() {
            let target_block = self.block(source_block);
            self.select_block(target_block);
            for instruction in source.block_instructions(source_block) {
                match instruction {
                    IRInstruction::ScopePush(_) | IRInstruction::ScopePop(_) => {}
                    IRInstruction::ExprDiscard(discard) => {
                        self.lower_expr_discard(context, discard).await;
                    }
                    IRInstruction::AddressDrop(drop) => {
                        self.lower_address_drop(context, drop).await;
                    }
                    IRInstruction::Expression(expression_id) => {
                        self.lower_expression(context, *expression_id, &source).await;
                    }
                    IRInstruction::Store(store) => {
                        let destination = self.lower_address(store.address());
                        let value = Rvalue::Use(self.expression_operand(store.expression()));
                        self.push_instruction(Instruction::Assign(Assign::new(destination, value)));
                    }
                }
            }
            let terminator = source
                .block_terminator(source_block)
                .expect("reachable semantic IR block should be terminated");
            self.lower_terminator(source_block, terminator, &source);
        }
    }

    async fn initialize_function_state(
        &mut self,
        context: &Context,
        source_id: IRFunctionID,
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
            self.initialize_environment_access(context, source_id, environment_parameter);
        }
    }

    fn initialize_environment_access(
        &mut self,
        context: &Context,
        source_id: IRFunctionID,
        environment_parameter: LocalID,
    ) {
        let environment_abi = context.function_environment_abi(source_id);
        let env = environment_abi.environment_type();
        if env.captures().is_empty() {
            return;
        }

        let environment_place = if environment_abi.by_value() {
            // no need to dereference the environment parameter, it is already the correct
            // type
            Place::new(environment_parameter)
        } else {
            let env_ty = context.intern_environment_type(env.clone());
            let pointer_type = context.create_pointer(env_ty, PointerMutability::Mut);
            let pointer_local =
                self.insert_local(Local::new(pointer_type.clone(), LocalKind::Temporary));

            // Case from the `void* env` parameter to the `Environment* env` local
            self.push_instruction(Instruction::Assign(Assign::new(
                Place::new(pointer_local),
                Rvalue::Cast(Cast::new(
                    Operand::Copy(Place::new(environment_parameter)),
                    pointer_type,
                )),
            )));

            // in order to access the environment fields, we need to dereference the pointer
            // local esentially, we create a `env->field` or `(*env).field` place
            // for each capture and handler in the environment
            Place::new(pointer_local).dereference()
        };

        for (index, capture_id) in environment_abi.capture_ids().enumerate() {
            self.insert_capture(
                capture_id,
                environment_place
                    .clone()
                    .project_environment_field(FieldIndex::new(index.try_into().unwrap())),
            );
        }

        // Initialize effect handlers from the same environment ABI used by its
        // producer.
        for (offset, effect) in environment_abi.captured_effects().cloned().enumerate() {
            let index = environment_abi.capture_count() + offset;
            self.insert_handler(
                effect,
                environment_place
                    .clone()
                    .project_environment_field(FieldIndex::new(index.try_into().unwrap())),
            );
        }

        assert_eq!(context.source_capture_count(source_id), environment_abi.capture_count());
    }
}
