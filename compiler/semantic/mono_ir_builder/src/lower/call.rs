use rayc_ir::ir_expr::{
    IRExprID,
    call::{Call as IRCall, CallTarget},
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoEffectInstance, MonoGeneratedAdtDropInstance,
    instance::FunctionReference,
    operand::Operand,
    place::{FieldIndex, Place},
    ty::{AggregateType, FunctionSignature, MonoType},
};

use crate::{
    function_lowering::{FunctionLowering, emitter::Emitter},
    resolver::InstanceCallable,
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
        let callee = Operand::Function(FunctionReference::NominalDrop(instance));
        self.call(Some(destination), callee, arguments);
    }
}

impl FunctionLowering<'_> {
    /// Calls a nominal closure with its inline environment and expanded Args
    /// tuple.
    fn lower_closure_call_in_callback_style(
        &mut self,
        call: &IRCall,
        instance: MonoClosureInstance,
        signature: &FunctionSignature,
        effects: &[MonoEffectInstance],
        expression_id: IRExprID,
    ) {
        // Def.call receives the nominal value and one evaluated Args tuple.
        assert_eq!(call.arguments().len(), 2);

        // The environment type must match
        let environment = self.expression_place(call.arguments()[0]);
        assert_eq!(self.expression_type(call.arguments()[0]), &signature.parameter_types()[0]);

        let tuple = self.expression_place(call.arguments()[1]);
        let tuple_type = self.expression_type(call.arguments()[1]).clone();
        let MonoType::Aggregate(AggregateType::Tuple(fields)) = &*tuple_type else {
            panic!("Def.call arguments must be a tuple")
        };

        assert_eq!(fields.fields().len() + 1 + effects.len(), signature.parameter_types().len());

        // Project the tuple temporary so argument expressions run once.
        let mut arguments = vec![Operand::Copy(environment)];
        for (index, field) in fields.fields().iter().enumerate() {
            assert_eq!(field, &signature.parameter_types()[index + 1]);
            arguments.push(Operand::Copy(
                self.project(&tuple).tuple_field(FieldIndex::from_usize(index)),
            ));
        }

        // Resolve handlers at the call site, outside the capture storage.
        arguments
            .extend(effects.iter().map(|effect| self.get_effect_instance_handler_operand(effect)));
        let callee = Operand::Function(FunctionReference::Closure(instance));

        self.call(Some(self.expression_place(expression_id)), callee, arguments);
    }

    pub(super) async fn lower_call_in_callback_style(
        &mut self,
        call: &IRCall,
        expression_id: IRExprID,
    ) {
        let arguments = call
            .arguments()
            .iter()
            .map(|argument| self.expression_operand(*argument))
            .collect::<Vec<_>>();

        let resolver = self.resolver();
        let destination = self.expression_place(expression_id);
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                self.apply_owner_substitution(&mut substitution);
                let callee = resolver.definition_instance(*function_id, substitution).await;
                self.lower_direct_call_in_callback_style(callee, arguments, destination).await;
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
    ) {
        match callee {
            InstanceCallable::Definition(callee) => {
                self.lower_direct_call_in_callback_style(callee, arguments, destination).await;
            }
            InstanceCallable::Closure(instance, signature, effects) => {
                self.lower_closure_call_in_callback_style(
                    call,
                    instance,
                    &signature,
                    &effects,
                    expression_id,
                );
            }
            InstanceCallable::TupleDrop(element_instances) => {
                assert_eq!(call.arguments().len(), 1);
                let tuple = self.expression_place(call.arguments()[0]);
                self.lower_tuple_drop(&tuple, &element_instances, destination).await;
            }
            InstanceCallable::ClosureDrop(capture_instances) => {
                assert_eq!(call.arguments().len(), 1);
                let closure = self.expression_place(call.arguments()[0]);
                self.lower_closure_drop(&closure, &capture_instances, destination).await;
            }
            InstanceCallable::NominalDrop(instance) => {
                assert_eq!(call.arguments().len(), 1);
                self.lower_nominal_drop_call(instance, arguments, destination);
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
        mut arguments: Vec<Operand>,
        destination: Place,
    ) {
        let callee_signature = self.resolver().def_signature(&callee).await;

        // appends additional effect handler arguments to the call
        for effect in callee_signature.effects() {
            arguments.push(self.get_effect_instance_handler_operand(effect));
        }

        self.lower_call_with_args(callee, arguments, destination).await;
    }
}
