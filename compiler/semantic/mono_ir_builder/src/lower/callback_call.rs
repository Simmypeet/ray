use rayc_ir::ir_expr::{
    IRExprID,
    call::{Call as IRCall, CallTarget},
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoGeneratedAdtDropInstance,
    instance::FunctionReference,
    instruction::Call,
    operand::Operand,
    place::{FieldIndex, Place},
    ty::FunctionSignature,
};

use crate::{
    context::Context, emitter::Emitter, lower::poll::PollSite, resolver::InstanceCallable,
};

impl Emitter<'_> {
    /// Calls a generated nominal Drop fragment. The fragment itself is
    /// discovered and lowered by the backend worklist.
    pub(crate) fn lower_nominal_drop_call(
        &mut self,
        instance: MonoGeneratedAdtDropInstance,
        arguments: Vec<Operand>,
        destination: Place,
    ) {
        self.push_call(
            Call::builder()
                .destination(destination)
                .callee(Operand::Function(FunctionReference::NominalDrop(instance)))
                .arguments(arguments)
                .build(),
        );
    }

    /// Calls a nominal closure with its inline environment and expanded Args
    /// tuple.
    fn lower_closure_call_in_callback_style(
        &mut self,
        call: &IRCall,
        instance: MonoClosureInstance,
        signature: &FunctionSignature,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        // Def.call receives the nominal value and one evaluated Args tuple.
        let [environment, tuple] = call.arguments() else {
            panic!("`Def.call` receives the closure and an argument tuple")
        };

        // The closure value is the environment that the body receives.
        let tuple_type = ctx.expression_type(*tuple);
        let tuple_type = tuple_type.assert_as_tuple();

        // Project the tuple temporary so argument expressions run once.
        let tuple = ctx.expression_place(*tuple);
        let arguments = (0..tuple_type.len())
            .map(|index| {
                Operand::Copy(self.project(&tuple).tuple_field(FieldIndex::from_usize(index)))
            })
            .collect();

        // Resolve handlers at the call site, outside the capture storage.
        let handler_records = signature
            .handler_record_slots()
            .iter()
            .map(|effect| ctx.get_effect_instance_handler_operand(effect))
            .collect();
        let callee = Operand::Function(FunctionReference::Closure(instance));

        self.push_call(
            Call::builder()
                .destination(ctx.expression_place(expression_id))
                .callee(callee)
                .arguments(arguments)
                .environment(Operand::Copy(ctx.expression_place(*environment)))
                .handler_records(handler_records)
                .build(),
        );
    }

    pub(super) async fn lower_call_in_callback_style(
        &mut self,
        call: &IRCall,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        let arguments = call
            .arguments()
            .iter()
            .map(|argument| ctx.expression_operand(*argument))
            .collect::<Vec<_>>();

        let resolver = self.resolver();
        let destination = ctx.expression_place(expression_id);
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                ctx.apply_owner_substitution(&mut substitution);
                let callee = resolver.definition_instance(*function_id, substitution).await;
                self.lower_direct_call_in_callback_style(callee, arguments, destination, ctx).await;
            }
            CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            } => {
                let callee =
                    resolver.resolve_instance_call(instance, *trait_def_id, trait_def_subst).await;

                self.lower_instance_call_in_callback_style(
                    call,
                    callee,
                    arguments,
                    destination,
                    expression_id,
                    ctx,
                )
                .await;
            }
        }
    }

    /// Calls what an instance call resolved to.
    async fn lower_instance_call_in_callback_style(
        &mut self,
        call: &IRCall,
        callee: InstanceCallable,
        arguments: Vec<Operand>,
        destination: Place,
        expression_id: IRExprID,
        ctx: &Context<'_>,
    ) {
        match callee {
            InstanceCallable::Definition(callee) => {
                self.lower_direct_call_in_callback_style(callee, arguments, destination, ctx).await;
            }
            InstanceCallable::Closure(instance, signature) => {
                self.lower_closure_call_in_callback_style(
                    call,
                    instance,
                    &signature,
                    expression_id,
                    ctx,
                );
            }
            InstanceCallable::TupleDrop(element_instances) => {
                assert_eq!(call.arguments().len(), 1);
                let tuple = ctx.expression_place(call.arguments()[0]);
                self.lower_tuple_drop(&tuple, &element_instances, destination).await;
            }
            InstanceCallable::ClosureDrop(captures) => {
                assert_eq!(call.arguments().len(), 1);
                let closure = ctx.expression_place(call.arguments()[0]);
                self.lower_closure_drop(&closure, &captures, destination).await;
            }
            InstanceCallable::NominalDrop(instance) => {
                assert_eq!(call.arguments().len(), 1);
                self.lower_nominal_drop_call(instance, arguments, destination);
            }
            // The call has blocks of its own, and what follows it continues
            // in a new one.
            InstanceCallable::Continuation(instance) => {
                let done = self.create_block();
                self.lower_continuation_call(
                    call,
                    instance,
                    PollSite::Inline,
                    destination,
                    done,
                    ctx,
                )
                .await;
                self.select_block(done);
            }
            // The arguments have already been evaluated by their own IR
            // expressions. A built-in no-op Drop call emits no instruction.
            InstanceCallable::NoOp => {}
        }
    }

    /// Calls a global definition, passing on this function's handlers.
    pub(crate) async fn lower_direct_call_in_callback_style(
        &mut self,
        callee: MonoDefInstance,
        arguments: Vec<Operand>,
        destination: Place,
        ctx: &Context<'_>,
    ) {
        let callee_signature = self.resolver().callback_def_signature(&callee).await;
        let handler_records = callee_signature
            .handler_record_slots()
            .iter()
            .map(|effect| ctx.get_effect_instance_handler_operand(effect))
            .collect();

        self.lower_call_with_args(callee, arguments, handler_records, destination).await;
    }
}
