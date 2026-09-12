use rayc_hash::FxHashMap;
use rayc_ir::ir_expr::{IRExprID, handle::Handle, perform::Perform};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::{Local, LocalID, LocalKind},
    instance::FunctionReference,
    operand::{FunctionOperand, Operand},
    place::Place,
    rvalue::{AddressOf, OperationHandlerSlot, Rvalue},
    ty::PointerMutability,
};
use rayc_symbol::GlobalSymbolID;

use crate::{builder::Builder, context::Context};

impl Builder<'_> {
    pub(super) fn lower_perform(
        &mut self,
        context: &Context,
        perform: &Perform,
        expression_id: IRExprID,
    ) {
        let instance = context.instantiate_effect(perform.effect_id(), perform.substitution());
        let handler = self.handler_place(&instance).dereference();

        // access the environment first
        let mut arguments = vec![Operand::Copy(
            handler.clone().project_operation_record_environment(perform.operation_id()),
        )];
        arguments
            .extend(perform.arguments().iter().map(|argument| self.expression_operand(*argument)));

        let destination = self.expression_place(expression_id);
        let callee = Operand::Copy(
            handler.project_operation_record_function_pointer(perform.operation_id()),
        );

        self.push_call_with_destination(destination, callee, arguments);
    }

    pub(super) fn lower_handle(
        &mut self,
        context: &Context,
        handle: &Handle,
        expression_id: IRExprID,
    ) {
        let instance = context.instantiate_effect(handle.effect_id(), handle.substitution());
        let handler_pointer = self.lower_effect_handler_pointer(context, handle, instance.clone());

        let body_abi = context.function_abi(handle.body().function_id());
        let mut arguments =
            vec![self.emit_opauqe_environment_pointer(context, handle.body().captures(), body_abi)];

        for effect in body_abi.effects() {
            if effect == &instance {
                arguments.push(Operand::Copy(Place::new(handler_pointer)));
            } else {
                arguments.push(self.handler_operand(effect));
            }
        }

        let destination = self.expression_place(expression_id);
        let callee = Operand::Function(FunctionOperand::new(
            FunctionReference::Local(context.target_function_id(handle.body().function_id())),
            body_abi.signature().clone(),
        ));
        self.push_call_with_destination(destination, callee, arguments);
    }

    pub(super) fn lower_effect_handler_pointer(
        &mut self,
        context: &Context,
        handle: &Handle,
        instance: MonoEffectInstance,
    ) -> LocalID {
        let handler_type = context.intern_effect_instance(instance.clone());
        let handler_local =
            self.insert_local(Local::new(handler_type.clone(), LocalKind::Temporary));
        let slots = self.lower_handler_slot(context, handle);

        self.assign(Place::new(handler_local), Rvalue::new_effect_handler(instance, slots));

        let handler_pointer_type = context.create_pointer(handler_type, PointerMutability::Const);
        let handler_pointer =
            self.insert_local(Local::new(handler_pointer_type, LocalKind::Temporary));
        self.assign(
            Place::new(handler_pointer),
            Rvalue::AddressOf(AddressOf::new(Place::new(handler_local), PointerMutability::Const)),
        );

        handler_pointer
    }

    pub(super) fn lower_handler_slot(
        &mut self,
        context: &Context,
        handle: &Handle,
    ) -> FxHashMap<GlobalSymbolID, OperationHandlerSlot> {
        let mut slots = FxHashMap::default();

        for handler in handle.handlers() {
            let abi = context.function_abi(handler.function_id());

            let env = self.emit_opauqe_environment_pointer(context, handler.captures(), abi);
            let fn_ptr = Operand::Function(FunctionOperand::new(
                FunctionReference::Local(context.target_function_id(handler.function_id())),
                abi.signature().clone(),
            ));

            assert!(
                slots
                    .insert(handler.operation_id(), OperationHandlerSlot::new(env, fn_ptr))
                    .is_none()
            );
        }

        slots
    }
}
