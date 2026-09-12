use rayc_ir::ir_expr::{
    IRExprID,
    call::{Call as IRCall, CallTarget},
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoEffectInstance,
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    place::FieldIndex,
    rvalue::Rvalue,
    ty::{AggregateType, FunctionSignature, MonoType},
};

use crate::{
    builder::Builder,
    context::{Context, InstanceCallable},
};

impl Builder<'_> {
    async fn lower_global_call(
        &mut self,
        context: &Context,
        callee: MonoDefInstance,
        mut arguments: Vec<Operand>,
        expression_id: IRExprID,
    ) {
        let (signature, effects, is_void) =
            context.global_signature(callee.def_id(), callee.substitution()).await;

        // appends additional effect handler arguments to the call
        for effect in effects {
            arguments.push(self.handler_operand(&effect));
        }

        let callee =
            Operand::Function(FunctionOperand::new(FunctionReference::Global(callee), signature));

        // if the function has `void` return type, which is mostly from `extern def`, we
        // don't need to assign the return value to the destination place
        let destination = (!is_void).then(|| self.expression_place(expression_id));

        self.push_instruction(Instruction::Call(Call::new(destination, callee, arguments)));

        // if we are calling a `void`  function, we need to assign "fake" unit value to
        // the destination place. (Actually, we don't need to assign anything, since
        // unit type has only one value, and we can just use uninitialized value)
        if is_void {
            self.assign(
                self.expression_place(expression_id),
                Rvalue::Use(Operand::Constant(Constant::Unit)),
            );
        }
    }

    /// Calls a nominal closure with its inline environment and expanded Args
    /// tuple.
    fn lower_closure_call(
        &mut self,
        call: &IRCall,
        instance: MonoClosureInstance,
        signature: FunctionSignature,
        effects: &[MonoEffectInstance],
        expression_id: IRExprID,
    ) {
        // Def.call receives the nominal value and one evaluated Args tuple.
        assert_eq!(call.arguments().len(), 2);

        // The environment type must match
        let environment = self.expression_place(call.arguments()[0]);
        assert_eq!(self.local_type(environment.local()), &signature.parameter_types()[0]);

        let tuple = self.expression_place(call.arguments()[1]);
        let tuple_type = self.local_type(tuple.local()).clone();
        let MonoType::Aggregate(AggregateType::Tuple(fields)) = &*tuple_type else {
            panic!("Def.call arguments must be a tuple")
        };

        assert_eq!(fields.fields().len() + 1 + effects.len(), signature.parameter_types().len());

        // Project the tuple temporary so argument expressions run once.
        let mut arguments = vec![Operand::Copy(environment)];
        for (index, field) in fields.fields().iter().enumerate() {
            assert_eq!(field, &signature.parameter_types()[index + 1]);
            arguments.push(Operand::Copy(
                tuple.clone().project_tuple_field(FieldIndex::new(index.try_into().unwrap())),
            ));
        }

        // Resolve handlers at the call site, outside the capture storage.
        arguments.extend(effects.iter().map(|effect| self.handler_operand(effect)));
        let callee = Operand::Function(FunctionOperand::new(
            FunctionReference::Closure(instance),
            signature,
        ));

        self.push_instruction(Instruction::Call(Call::new(
            Some(self.expression_place(expression_id)),
            callee,
            arguments,
        )));
    }

    /// Calls an erased lambda through its stored function pointer and
    /// environment.
    fn lower_lambda_call(
        &mut self,
        callee: IRExprID,
        mut arguments: Vec<Operand>,
        expression_id: IRExprID,
    ) {
        let callee_place = self.expression_place(callee);
        let callee_type = self.local_type(callee_place.local()).clone();
        let closure = callee_type.assert_as_closure();

        let handler_offset = 1 + arguments.len();
        let effects = closure
            .function_signature()
            .parameter_types()
            .iter()
            .skip(handler_offset)
            .map(|handler_type| {
                let pointer = handler_type.assert_as_pointer();
                let handler = pointer.pointee().assert_as_effect_handler();
                handler.mono_effect_instance().clone()
            })
            .collect::<Vec<_>>();

        // we're generating something like `callee.environment, ...args`
        arguments.insert(0, Operand::Copy(callee_place.clone().project_closure_environment()));

        for effect in effects {
            arguments.push(self.handler_operand(&effect));
        }

        let code = Operand::Copy(callee_place.project_closure_function_pointer());
        let destination = self.expression_place(expression_id);

        self.push_instruction(Instruction::Call(Call::new(Some(destination), code, arguments)));
    }

    pub(super) async fn lower_call(
        &mut self,
        context: &Context,
        call: &IRCall,
        expression_id: IRExprID,
    ) {
        let arguments = call
            .arguments()
            .iter()
            .map(|argument| self.expression_operand(*argument))
            .collect::<Vec<_>>();

        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                context.apply_owner_substitution(&mut substitution);
                let callee = context.definition_instance(*function_id, substitution).await;
                self.lower_global_call(context, callee, arguments, expression_id).await;
            }
            CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            } => {
                let callee =
                    context.resolve_instance_call(instance, *trait_def_id, trait_def_subst).await;
                match callee {
                    InstanceCallable::Definition(callee) => {
                        self.lower_global_call(context, callee, arguments, expression_id).await;
                    }
                    InstanceCallable::Closure(instance, signature, effects) => {
                        self.lower_closure_call(call, instance, signature, &effects, expression_id);
                    }
                }
            }
            CallTarget::Lambda { callee } => {
                self.lower_lambda_call(*callee, arguments, expression_id);
            }
        }
    }
}
