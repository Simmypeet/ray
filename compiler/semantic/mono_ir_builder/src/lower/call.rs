use qbice::storage::intern::Interned;
use rayc_ir::{
    cfg::ExprDiscard,
    ir_expr::{
        IRExprID,
        call::{Call as IRCall, CallTarget},
    },
};
use rayc_mono_ir::{
    MonoClosureInstance, MonoDefInstance, MonoEffectInstance, MonoNominalDropInstance,
    function::{Local, LocalKind},
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    place::{FieldIndex, Place},
    rvalue::Rvalue,
    ty::{AggregateType, FunctionSignature, MonoType},
};
use rayc_type::ty::Ty;

use crate::{
    builder::Builder,
    context::Context,
    resolver::{InstanceCallable, Resolver},
};

impl Builder<'_> {
    /// Drops `value` with the implementation selected by `dictionary`. Every
    /// emitted call writes its unit result to `destination`.
    pub(crate) async fn lower_drop(
        &mut self,
        resolver: &Resolver,
        value: Place,
        dictionary: &Interned<Ty>,
        destination: Place,
    ) {
        match resolver.resolve_drop_call(dictionary).await {
            InstanceCallable::Definition(callee) => {
                self.lower_global_call(resolver, callee, vec![Operand::Copy(value)], destination)
                    .await;
            }
            InstanceCallable::TupleDrop(element_instances) => {
                Box::pin(self.lower_tuple_drop(resolver, value, &element_instances, destination))
                    .await;
            }
            InstanceCallable::ClosureDrop(capture_instances) => {
                Box::pin(self.lower_closure_drop(resolver, value, &capture_instances, destination))
                    .await;
            }
            InstanceCallable::NominalDrop(instance, signature) => {
                self.lower_nominal_drop_call(
                    instance,
                    signature,
                    vec![Operand::Copy(value)],
                    destination,
                );
            }
            InstanceCallable::NoOp => {}
            InstanceCallable::Closure(_, _, _) => {
                panic!("a Drop dictionary cannot call a closure")
            }
        }
    }

    /// Drops the unused value of an expression statement with the dictionary
    /// selected during type checking.
    pub(super) async fn lower_expr_discard(&mut self, context: &Context, discard: &ExprDiscard) {
        let resolver = context.resolver();

        // `Drop.drop` returns unit, which is itself unused, so every call
        // writes to one scratch temporary.
        let unit = resolver.unit_type().await;
        let destination = Place::new(self.insert_local(Local::new(unit, LocalKind::Temporary)));

        let value = self.expression_place(discard.expression());
        self.lower_drop(resolver, value, discard.drop_instance(), destination).await;
    }

    /// Expands a built-in tuple `Drop` call into the selected element calls.
    async fn lower_tuple_drop(
        &mut self,
        resolver: &Resolver,
        tuple: Place,
        element_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        // Destroy fields in reverse declaration order, matching scope teardown.
        for (index, element_instance) in element_instances.iter().enumerate().rev() {
            let field =
                tuple.clone().project_tuple_field(FieldIndex::new(index.try_into().unwrap()));
            self.lower_drop(resolver, field, element_instance, destination.clone()).await;
        }
    }

    /// Expands a built-in closure `Drop` call into the selected capture calls.
    async fn lower_closure_drop(
        &mut self,
        resolver: &Resolver,
        closure: Place,
        capture_instances: &[Interned<Ty>],
        destination: Place,
    ) {
        // A closure value is its inline environment, whose fields are the
        // captures. Destroy them in reverse order, matching tuple Drop.
        for (index, capture_instance) in capture_instances.iter().enumerate().rev() {
            let capture = closure
                .clone()
                .project_environment_field(FieldIndex::new(index.try_into().unwrap()));
            self.lower_drop(resolver, capture, capture_instance, destination.clone()).await;
        }
    }

    /// Calls a generated nominal Drop fragment. The fragment itself is
    /// discovered and lowered by the backend worklist.
    fn lower_nominal_drop_call(
        &mut self,
        instance: MonoNominalDropInstance,
        signature: FunctionSignature,
        arguments: Vec<Operand>,
        destination: Place,
    ) {
        let callee = Operand::Function(FunctionOperand::new(
            FunctionReference::NominalDrop(instance),
            signature,
        ));
        self.push_instruction(Instruction::Call(Call::new(Some(destination), callee, arguments)));
    }

    async fn lower_global_call(
        &mut self,
        resolver: &Resolver,
        callee: MonoDefInstance,
        mut arguments: Vec<Operand>,
        destination: Place,
    ) {
        let (signature, effects, is_void) =
            resolver.global_signature(callee.def_id(), callee.substitution()).await;

        // appends additional effect handler arguments to the call
        for effect in effects {
            arguments.push(self.handler_operand(&effect));
        }

        let callee =
            Operand::Function(FunctionOperand::new(FunctionReference::Global(callee), signature));

        // if the function has `void` return type, which is mostly from `extern def`, we
        // don't need to assign the return value to the destination place
        let call_destination = (!is_void).then(|| destination.clone());

        self.push_instruction(Instruction::Call(Call::new(call_destination, callee, arguments)));

        // if we are calling a `void`  function, we need to assign "fake" unit value to
        // the destination place. (Actually, we don't need to assign anything, since
        // unit type has only one value, and we can just use uninitialized value)
        if is_void {
            self.assign(destination, Rvalue::Use(Operand::Constant(Constant::Unit)));
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

        let resolver = context.resolver();
        let destination = self.expression_place(expression_id);
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                context.apply_owner_substitution(&mut substitution);
                let callee = context.definition_instance(*function_id, substitution).await;
                self.lower_global_call(resolver, callee, arguments, destination).await;
            }
            CallTarget::UnresolvedInstanceAssociated {
                instance,
                trait_def_id,
                trait_def_subst,
            } => {
                let callee =
                    resolver.resolve_instance_call(instance, *trait_def_id, trait_def_subst).await;
                match callee {
                    InstanceCallable::Definition(callee) => {
                        self.lower_global_call(resolver, callee, arguments, destination).await;
                    }
                    InstanceCallable::Closure(instance, signature, effects) => {
                        self.lower_closure_call(call, instance, signature, &effects, expression_id);
                    }
                    InstanceCallable::TupleDrop(element_instances) => {
                        assert_eq!(call.arguments().len(), 1);
                        let tuple = self.expression_place(call.arguments()[0]);
                        self.lower_tuple_drop(resolver, tuple, &element_instances, destination)
                            .await;
                    }
                    InstanceCallable::ClosureDrop(capture_instances) => {
                        assert_eq!(call.arguments().len(), 1);
                        let closure = self.expression_place(call.arguments()[0]);
                        self.lower_closure_drop(resolver, closure, &capture_instances, destination)
                            .await;
                    }
                    InstanceCallable::NominalDrop(instance, signature) => {
                        assert_eq!(call.arguments().len(), 1);
                        self.lower_nominal_drop_call(instance, signature, arguments, destination);
                    }
                    // The arguments have already been evaluated by their own IR
                    // expressions. A built-in no-op Drop call emits no instruction.
                    InstanceCallable::NoOp => {}
                }
            }
        }
    }
}
