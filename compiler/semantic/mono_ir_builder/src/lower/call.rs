use rayc_ir::ir_expr::{
    IRExprID,
    call::{Call as IRCall, CallTarget},
};
use rayc_mono_ir::{
    MonoDefInstance,
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    rvalue::Rvalue,
};

use crate::{builder::Builder, context::Context};

impl Builder<'_> {
    pub(super) async fn lower_call(
        &mut self,
        context: &Context,
        call: &IRCall,
        expression_id: IRExprID,
    ) {
        let mut arguments = call
            .arguments()
            .iter()
            .map(|argument| self.expression_operand(*argument))
            .collect::<Vec<_>>();

        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                context.apply_owner_substitution(&mut substitution);

                let (signature, effects, is_void) =
                    context.global_signature(*function_id, &substitution).await;

                // appends additional effect handler arguments to the call
                for effect in effects {
                    arguments.push(self.handler_operand(&effect));
                }

                let callee = Operand::Function(FunctionOperand::new(
                    FunctionReference::Global(MonoDefInstance::new(*function_id, substitution)),
                    signature,
                ));

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
            CallTarget::UnresolvedInstanceAssociated { .. } => todo!(),
            CallTarget::Lambda { callee } => {
                let callee_place = self.expression_place(*callee);
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
                arguments
                    .insert(0, Operand::Copy(callee_place.clone().project_closure_environment()));

                for effect in effects {
                    arguments.push(self.handler_operand(&effect));
                }

                let code = Operand::Copy(callee_place.project_closure_function_pointer());
                let destination = self.expression_place(expression_id);

                self.push_instruction(Instruction::Call(Call::new(
                    Some(destination),
                    code,
                    arguments,
                )));
            }
        }
    }
}
