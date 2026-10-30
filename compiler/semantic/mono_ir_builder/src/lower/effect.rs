use std::collections::BTreeMap;

use rayc_ir::ir_expr::{IRExprID, perform::Perform, run_with::RunWith};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::LocalID,
    instance::FunctionReference,
    operand::Operand,
    place::FieldIndex,
    rvalue::{AddressOf, OperationHandlerSlot, Rvalue},
    ty::{AggregateType, EffectHandler, MonoType, PointerMutability},
};
use rayc_symbol::GlobalSymbolID;

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    pub(super) async fn lower_perform(&mut self, perform: &Perform, expression_id: IRExprID) {
        let instance = self.instantiate_effect(perform.effect_id(), perform.substitution()).await;
        let handler_pointer = self.handler_place(&instance);
        let handler = self.project(&handler_pointer).dereference();
        let projector = self.project(&handler);

        // access the environment first
        let mut arguments =
            vec![Operand::Copy(projector.operation_record_environment(perform.operation_id()))];
        arguments
            .extend(perform.arguments().iter().map(|argument| self.expression_operand(*argument)));

        let callee =
            Operand::Copy(projector.operation_record_function_pointer(perform.operation_id()));

        self.call(Some(self.expression_place(expression_id)), callee, arguments);
    }

    pub(super) async fn lower_run_with(&mut self, run_with: &RunWith, expression_id: IRExprID) {
        // Its nested functions live in the fragment of the owning definition,
        // which a coroutine's fragment cannot refer to yet.
        assert!(!self.is_coroutine(), "`run ... with` in a coroutine is not lowered to MonoIR yet");

        let instance = self.instantiate_effect(run_with.effect_id(), run_with.substitution()).await;
        let (handler_pointer, handler_environment) =
            self.lower_effect_handler_pointer(run_with, instance.clone());

        let body_id = run_with.body().function_id();
        let body_abi = self.function_abi(body_id);
        let body_environment = self.function_environment_abi(body_id);
        let mut arguments = vec![
            self.emit_opaque_environment_pointer(run_with.body().captures(), body_environment),
        ];

        for effect in body_abi.effects() {
            if effect == &instance {
                arguments.push(Operand::Copy(self.place(handler_pointer)));
            } else {
                arguments.push(self.get_effect_instance_handler_operand(effect));
            }
        }

        let callee = Operand::Function(FunctionReference::Local(self.target_function_id(body_id)));
        self.call(Some(self.expression_place(expression_id)), callee, arguments);

        // The handlers only borrow their shared captures, so they are dropped
        // once the handled body has returned.
        if let Some(environment) = handler_environment {
            self.drop_handler_captures(run_with, environment).await;
        }
    }

    /// Drops the captures stored in a handler environment, in reverse order,
    /// with the dictionaries memory analysis selected for them.
    async fn drop_handler_captures(&mut self, run_with: &RunWith, environment: LocalID) {
        let drops = run_with.handler_capture_drops();
        assert_eq!(
            drops.len(),
            run_with.handler_captures().len(),
            "memory analysis selects a Drop dictionary for every handler capture"
        );

        // `Drop.drop` returns unit, which is itself unused, so every call
        // writes to one scratch temporary.
        let destination = self.scratch_unit();

        // The captures come first in the environment, followed by any
        // captured effect handlers, which need no drop.
        let environment = self.place(environment);
        for (index, drop_instance) in drops.iter().enumerate().rev() {
            let capture =
                self.project(&environment).environment_field(FieldIndex::from_usize(index));
            self.lower_drop(capture, drop_instance, destination.clone()).await;
        }
    }

    fn lower_effect_handler_pointer(
        &mut self,
        run_with: &RunWith,
        instance: MonoEffectInstance,
    ) -> (LocalID, Option<LocalID>) {
        let engine = self.engine();
        let handler_type = MonoType::new_aggregate(
            AggregateType::EffectHandler(EffectHandler::new(instance.clone())),
            engine,
        );
        let handler = self.temporary(handler_type.clone());
        let (slots, environment) = self.lower_handler_slot(run_with);
        self.assign(handler.clone(), Rvalue::new_effect_handler(instance, slots));

        let pointer_type = MonoType::new_pointer(handler_type, PointerMutability::Const, engine);
        let pointer_local = self.temporary_local(pointer_type);
        self.assign(
            self.place(pointer_local),
            Rvalue::AddressOf(AddressOf::new(handler, PointerMutability::Const)),
        );

        (pointer_local, environment)
    }

    /// Builds the operation slots of a handler record, and returns them with
    /// the local of the environment the handlers share, if it has captures.
    fn lower_handler_slot(
        &mut self,
        run_with: &RunWith,
    ) -> (BTreeMap<GlobalSymbolID, OperationHandlerSlot>, Option<LocalID>) {
        let mut slots = BTreeMap::new();
        let Some(capture_map_id) = run_with.handler_capture_map() else {
            assert!(run_with.handlers().is_empty());
            return (slots, None);
        };

        // Construct the environment exactly once from the handler group's
        // arena-owned ABI.
        let environment_abi = self.capture_environment_abi(capture_map_id);
        let (environment, shared_environment) =
            self.emit_environment(run_with.handler_captures(), environment_abi);

        for handler in run_with.handlers() {
            self.assert_function_uses_capture_environment(handler.function_id(), capture_map_id);
            let fn_ptr = Operand::Function(FunctionReference::Local(
                self.target_function_id(handler.function_id()),
            ));

            assert!(
                slots
                    .insert(
                        handler.operation_id(),
                        OperationHandlerSlot::new(shared_environment.clone(), fn_ptr),
                    )
                    .is_none()
            );
        }

        (slots, environment)
    }
}
