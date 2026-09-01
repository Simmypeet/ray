use rayc_hash::FxHashMap;
use rayc_ir::ir_expr::{IRExprID, handle::Handle, perform::Perform};
use rayc_mono_ir::{
    cfg::BlockID,
    function::{Local, LocalKind},
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{FunctionOperand, Operand},
    place::{FieldIndex, Place},
    rvalue::{AddressOf, AggregateValue, Rvalue},
    ty::{
        AggregateType, EffectHandler, MonoType, PointerMutability, build_handler_layout,
        instantiate_effect,
    },
};

use super::{Builder, BuilderState};

impl Builder {
    pub(super) async fn lower_perform(
        &mut self,
        perform: &Perform,
        expression_id: IRExprID,
        block: BlockID,
        builder_state: &mut BuilderState<'_>,
    ) {
        let instance = self.engine.instantiate_effect(
            perform.effect_id(),
            perform.substitution(),
            self.instance.substitution(),
        );
        let layout = self.engine.build_handler_layout(instance.clone()).await;
        let (slot, operation) = layout
            .operations()
            .iter()
            .enumerate()
            .find(|(_, operation)| operation.operation_id() == perform.operation_id())
            .expect("performed operation should belong to its effect");
        let handler = builder_state.handler_place(&instance).dereference();
        let operation_index = FieldIndex::new(slot.try_into().unwrap());
        let mut arguments = vec![Operand::Copy(
            handler.clone().project_operation_record_environment(operation_index),
        )];
        arguments.extend(
            perform.arguments().iter().map(|argument| builder_state.expression_operand(*argument)),
        );
        let _ = operation.signature();
        let destination = builder_state.expression_place(expression_id);
        builder_state.push_instruction(
            block,
            Instruction::Call(Call::new(
                Some(destination),
                Operand::Copy(handler.project_operation_record_function_pointer(operation_index)),
                arguments,
            )),
        );
    }

    pub(super) async fn lower_handle(
        &mut self,
        handle: &Handle,
        expression_id: IRExprID,
        block: BlockID,
        builder_state: &mut BuilderState<'_>,
    ) {
        let instance = self.engine.instantiate_effect(
            handle.effect_id(),
            handle.substitution(),
            self.instance.substitution(),
        );
        let layout = self.engine.build_handler_layout(instance.clone()).await;
        let handlers = handle
            .handlers()
            .iter()
            .map(|handler| (handler.operation_id(), handler.function()))
            .collect::<FxHashMap<_, _>>();
        let mut slots = Vec::with_capacity(layout.operations().len());
        for operation in layout.operations() {
            let handled = handlers
                .get(&operation.operation_id())
                .expect("complete handler should define every operation");
            let abi = self.function_abi(handled.function_id()).clone();
            assert_eq!(
                abi.signature(),
                operation.signature(),
                "operation handler callback should match its effect operation"
            );
            let environment = self.emit_environment(handled, &abi, block, builder_state);
            let closure_type = self.closure_type(abi.signature().clone());
            let closure_local =
                builder_state.insert_local(Local::new(closure_type.clone(), LocalKind::Temporary));
            Self::assign(
                block,
                Place::new(closure_local),
                Rvalue::Aggregate(AggregateValue::new(closure_type, vec![
                    environment,
                    Operand::Function(FunctionOperand::new(
                        FunctionReference::Local(builder_state.target_id()),
                        abi.signature().clone(),
                    )),
                ])),
                builder_state,
            );
            slots.push(Operand::Copy(Place::new(closure_local)));
        }

        let handler_type = self.engine.intern(MonoType::Aggregate(AggregateType::EffectHandler(
            EffectHandler::new(instance.clone()),
        )));
        let handler_local =
            builder_state.insert_local(Local::new(handler_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(handler_local),
            Rvalue::Aggregate(AggregateValue::new(handler_type.clone(), slots)),
            builder_state,
        );
        let handler_pointer_type =
            MonoType::new_pointer(handler_type, PointerMutability::Const, &self.engine);
        let handler_pointer =
            builder_state.insert_local(Local::new(handler_pointer_type, LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(handler_pointer),
            Rvalue::AddressOf(AddressOf::new(Place::new(handler_local), PointerMutability::Const)),
            builder_state,
        );

        let body_abi = self.function_abi(handle.body().function_id()).clone();
        let mut arguments =
            vec![self.emit_environment(handle.body(), &body_abi, block, builder_state)];
        for effect in body_abi.effects() {
            if effect == &instance {
                arguments.push(Operand::Copy(Place::new(handler_pointer)));
            } else {
                arguments.push(builder_state.handler_operand(effect));
            }
        }
        let destination = builder_state.expression_place(expression_id);
        builder_state.push_instruction(
            block,
            Instruction::Call(Call::new(
                Some(destination),
                Operand::Function(FunctionOperand::new(
                    FunctionReference::Local(builder_state.target_id()),
                    body_abi.signature().clone(),
                )),
                arguments,
            )),
        );
    }
}
