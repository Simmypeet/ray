//! Lowers `perform` and `run ... with` in callback style, where an effect is
//! a record of callbacks that the function holds a pointer to, and an
//! operation is a call of one of them, which returns to the function.
//!
//! A function other than an operation handler receives the handler of each
//! effect of its row as a parameter after its source parameters. An operation
//! handler finds them in fields of its environment after its captures.
//!
//! A coroutine yields its operations instead, so neither is lowered here for
//! one.

use rayc_ir::ir_expr::{IRExprID, perform::Perform, run_with::RunWith};
use rayc_mono_ir::{
    MonoEffectInstance,
    function::LocalID,
    instance::FunctionReference,
    instruction::Call,
    operand::Operand,
    place::Projection,
    rvalue::{AddressOf, Rvalue},
    ty::{AggregateType, EffectHandlerCallback, MonoType, PointerMutability},
};
use rayc_symbol::GlobalSymbolID;

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers a `perform` to a call of the callback of its operation in this
    /// function's handler of the effect, with the environment stored next to
    /// it:
    ///
    /// ```c
    /// // perform E.op(a, b)
    /// result = handler_E->op.function(handler_E->op.environment, a, b);
    /// ```
    pub(super) async fn lower_callback_perform(
        &mut self,
        perform: &Perform,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        let instance = ctx.instantiate_effect(perform.effect_id(), perform.substitution()).await;
        let (fn_callee, environment) =
            self.handler_operation_callback(&instance, perform.operation_id(), ctx);

        let arguments =
            perform.arguments().iter().map(|argument| ctx.expression_operand(*argument)).collect();
        self.call(
            Call::new(Some(ctx.expression_place(expression_id)), fn_callee, arguments)
                .with_environment(environment),
        );
    }

    /// The callback of `operation_id` in this function's handler of `effect`,
    /// with the environment that is its first argument.
    pub(super) fn handler_operation_callback(
        &self,
        effect: &MonoEffectInstance,
        operation_id: GlobalSymbolID,
        ctx: &Context<'_>,
    ) -> (Operand, Operand) {
        let handler_pointer = ctx.handler_place(effect);
        let handler = self.project(&handler_pointer).dereference();
        let projector = self.project(&handler);

        (
            Operand::Copy(projector.operation_record_function_pointer(operation_id)),
            Operand::Copy(projector.operation_record_environment(operation_id)),
        )
    }

    /// Lowers a `run ... with` to a call of its body, a thunk in the same
    /// fragment, with the handler record built in place of this function's
    /// handler of the handled effect:
    ///
    /// ```c
    /// // run { body } with E { op(x) { clause } }
    /// const E_handler_t *handler_pointer = ...;  // lower_effect_handler_pointer
    ///
    /// // The body takes its environment, then one handler per effect of its
    /// // row: the record for `E`, and this function's own for the others.
    /// body_env_t body_env = { capture_0, ... };
    /// result = body_thunk((void *)&body_env, handler_pointer, handler_F);
    ///
    /// drop(handler_env.capture_n); ...;          // drop_handler_captures
    /// ```
    ///
    /// where the body is
    ///
    /// ```c
    /// R body_thunk(void *env, const E_handler_t *handler_E, const F_handler_t *handler_F);
    /// ```
    pub(super) async fn lower_callback_run_with(
        &mut self,
        run_with: &RunWith,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        // Its nested functions live in the fragment of the owning definition,
        // which a coroutine's fragment cannot refer to yet.
        assert!(!ctx.is_coroutine(), "`run ... with` in a coroutine is not lowered to MonoIR yet");

        let instance = ctx.instantiate_effect(run_with.effect_id(), run_with.substitution()).await;
        let (handler_pointer, handler_environment) =
            self.lower_effect_handler_pointer(run_with, instance.clone(), ctx);

        let body_id = run_with.body().function_id();
        let body_abi = ctx.function_abi(body_id);
        let body_environment = ctx.function_environment_abi(body_id);
        let environment =
            self.emit_opaque_environment_pointer(run_with.body().captures(), body_environment, ctx);

        // The new record handles its effect, and this function's own handlers
        // the others.
        let handler_records = body_abi
            .handler_record_slots()
            .iter()
            .map(|effect| {
                if effect == &instance {
                    Operand::Copy(self.place(handler_pointer))
                } else {
                    ctx.get_effect_instance_handler_operand(effect)
                }
            })
            .collect();

        let callee = Operand::Function(FunctionReference::Local(ctx.target_function_id(body_id)));
        self.call(
            Call::new(Some(ctx.expression_place(expression_id)), callee, Vec::new())
                .with_environment(environment)
                .with_handler_records(handler_records),
        );

        // The handlers only borrow their shared captures, so they are dropped
        // once the handled body has returned.
        if let Some(environment) = handler_environment {
            self.drop_handler_captures(run_with, environment, ctx).await;
        }
    }

    /// Drops the captures stored in a handler environment, in reverse order,
    /// with the dictionaries memory analysis selected for them. The clauses
    /// only borrow their captures, so this runs once the body has returned:
    ///
    /// ```c
    /// drop(handler_env.capture_n); ...; drop(handler_env.capture_0);
    /// ```
    async fn drop_handler_captures(
        &mut self,
        run_with: &RunWith,
        environment: LocalID,
        ctx: &Context<'_>,
    ) {
        let drops = run_with.handler_capture_drops();

        // `Drop.drop` returns unit, which is itself unused, so every call
        // writes to one scratch temporary.
        let destination = self.new_unit_place();

        // Drop the captures in reverse order of the capture map. The handler
        // records of the environment need no drop.
        let capture_map_id =
            run_with.handler_capture_map().expect("an environment comes from a capture map");
        let environment_abi = ctx.capture_environment_abi(capture_map_id);
        let environment = self.place(environment);
        for capture in environment_abi.capture_ids().rev() {
            let drop_instance = drops
                .get(&capture)
                .expect("memory analysis selects a Drop dictionary for every handler capture");
            let capture = self.project(&environment).environment_capture(capture);
            self.lower_drop(capture, drop_instance, destination.clone()).await;
        }
    }

    /// Builds the handler record of a `run ... with` on the stack, and returns
    /// the local of a pointer to it with the local of the environment its
    /// clauses share, if it has captures:
    ///
    /// ```c
    /// E_handler_t handler = { ... };              // lower_handler_members
    /// const E_handler_t *handler_pointer = &handler;
    /// ```
    fn lower_effect_handler_pointer(
        &mut self,
        run_with: &RunWith,
        instance: MonoEffectInstance,
        ctx: &Context<'_>,
    ) -> (LocalID, Option<LocalID>) {
        let engine = self.engine();
        let aggregate = AggregateType::EffectHandlerCallback(EffectHandlerCallback::new(instance));
        let handler_type = MonoType::new_aggregate(aggregate, engine);
        let handler = self.new_temporary_place(handler_type.clone());

        let (members, environment) = self.lower_handler_members(run_with, ctx);
        self.initialize_aggregate(&handler, members);

        let pointer_type = MonoType::new_pointer(handler_type, PointerMutability::Const, engine);
        let pointer_local = self.new_temporary_local(pointer_type);
        self.assign(
            self.place(pointer_local),
            Rvalue::AddressOf(AddressOf::new(handler, PointerMutability::Const)),
        );

        (pointer_local, environment)
    }

    /// Builds the members of a handler record, and returns them with the local
    /// of the environment the handlers share, if it has captures. Each
    /// operation is a closure: an environment and a callback.
    ///
    /// ```c
    /// // The clauses share one environment: their captures, then the
    /// // handlers of the effects the clauses perform. Without captures it is
    /// // NULL.
    /// handler_env_t handler_env = { capture_0, ..., handler_F };
    /// void *handler_env_pointer = (void *)&handler_env;
    ///
    /// // One callback per operation, each with the shared environment.
    /// { .op = { .environment = handler_env_pointer, .function = op_handler } }
    /// ```
    ///
    /// where each clause is
    ///
    /// ```c
    /// T op_handler(void *env, X x); // its handlers are in `env`, after the captures
    /// ```
    fn lower_handler_members(
        &mut self,
        run_with: &RunWith,
        ctx: &Context<'_>,
    ) -> (Vec<(Projection, Operand)>, Option<LocalID>) {
        let Some(capture_map_id) = run_with.handler_capture_map() else {
            assert!(run_with.handlers().is_empty());
            return (Vec::new(), None);
        };

        // Construct the environment exactly once from the handler group's
        // arena-owned ABI.
        let environment_abi = ctx.capture_environment_abi(capture_map_id);
        let (environment, shared_environment) =
            self.emit_environment(run_with.handler_captures(), environment_abi, ctx);

        let mut members = Vec::with_capacity(run_with.handlers().len() * 2);
        for handler in run_with.handlers() {
            ctx.assert_function_uses_capture_environment(handler.function_id(), capture_map_id);
            let fn_ptr = Operand::Function(FunctionReference::Local(
                ctx.target_function_id(handler.function_id()),
            ));

            let operation = handler.operation_id();

            members.push((
                Projection::OperationRecordEnvironmentField(operation),
                shared_environment.clone(),
            ));
            members.push((Projection::OperationRecordFunctionPointerField(operation), fn_ptr));
        }

        (members, environment)
    }
}
