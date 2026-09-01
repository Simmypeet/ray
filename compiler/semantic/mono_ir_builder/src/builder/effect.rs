use rayc_hash::FxHashMap;
use rayc_ir::ir_expr::{IRExprID, handle::Handle, perform::Perform};
use rayc_mono_ir::{
    MonoIR,
    cfg::BlockID,
    function::{Local, LocalKind, MonoFunctionID},
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{FunctionOperand, Operand},
    place::{FieldIndex, Place},
    rvalue::{AddressOf, AggregateValue, Rvalue},
    ty::{MonoType, PointerMutability},
};

use super::{Builder, FunctionState};

impl Builder {
    pub(super) async fn lower_perform(
        &mut self,
        perform: &Perform,
        expression_id: IRExprID,
        block: BlockID,
        state: &FunctionState,
        output: &mut MonoIR,
    ) {
        let instance = self.types.effect_instance(
            perform.effect_id(),
            perform.substitution(),
            self.instance.substitution(),
        );
        let layout = self.types.ensure_handler_layout(&instance).await.clone();
        let (slot, operation) = layout
            .operations()
            .iter()
            .enumerate()
            .find(|(_, operation)| operation.operation_id() == perform.operation_id())
            .expect("performed operation should belong to its effect");
        let handler = state.handler_place(&instance).dereference();
        let closure = handler.project_field(FieldIndex::new(slot.try_into().unwrap()));
        let mut arguments = vec![Operand::Copy(closure.clone().project_field(FieldIndex::new(1)))];
        arguments
            .extend(perform.arguments().iter().map(|argument| state.expression_operand(*argument)));
        let _ = operation.signature();
        output.push_instruction(
            state.target_id,
            block,
            Instruction::Call(Call::new(
                Some(state.expression_place(expression_id)),
                Operand::Copy(closure.project_field(FieldIndex::new(0))),
                arguments,
            )),
        );
    }

    pub(super) async fn lower_handle(
        &mut self,
        handle: &Handle,
        expression_id: IRExprID,
        block: BlockID,
        target_id: MonoFunctionID,
        state: &mut FunctionState,
        output: &mut MonoIR,
    ) {
        let instance = self.types.effect_instance(
            handle.effect_id(),
            handle.substitution(),
            self.instance.substitution(),
        );
        let layout = self.types.ensure_handler_layout(&instance).await.clone();
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
            let environment = self.emit_environment(handled, &abi, block, state, output);
            let closure_type = self.closure_type(abi.signature().clone());
            let closure_local = output.insert_local(
                state.target_id,
                Local::new(closure_type.clone(), LocalKind::Temporary),
            );
            Self::assign(
                block,
                Place::new(closure_local),
                Rvalue::Aggregate(AggregateValue::new(closure_type, vec![
                    Operand::Function(FunctionOperand::new(
                        FunctionReference::Local(target_id),
                        abi.signature().clone(),
                    )),
                    environment,
                ])),
                state,
                output,
            );
            slots.push(Operand::Copy(Place::new(closure_local)));
        }

        let handler_type = self.types.intern(MonoType::EffectHandler(instance.clone()));
        let handler_local = output
            .insert_local(state.target_id, Local::new(handler_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(handler_local),
            Rvalue::Aggregate(AggregateValue::new(handler_type.clone(), slots)),
            state,
            output,
        );
        let handler_pointer_type = self.types.pointer(handler_type, PointerMutability::Const);
        let handler_pointer = output
            .insert_local(state.target_id, Local::new(handler_pointer_type, LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(handler_pointer),
            Rvalue::AddressOf(AddressOf::new(Place::new(handler_local), PointerMutability::Const)),
            state,
            output,
        );

        let body_abi = self.function_abi(handle.body().function_id()).clone();
        let mut arguments =
            vec![self.emit_environment(handle.body(), &body_abi, block, state, output)];
        for effect in body_abi.effects() {
            if effect == &instance {
                arguments.push(Operand::Copy(Place::new(handler_pointer)));
            } else {
                arguments.push(state.handler_operand(effect));
            }
        }
        output.push_instruction(
            state.target_id,
            block,
            Instruction::Call(Call::new(
                Some(state.expression_place(expression_id)),
                Operand::Function(FunctionOperand::new(
                    FunctionReference::Local(target_id),
                    body_abi.signature().clone(),
                )),
                arguments,
            )),
        );
    }
}
