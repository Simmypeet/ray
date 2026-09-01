use rayc_ir::{
    ir_expr::{
        IRExprID,
        call::{Call as IRCall, CallTarget},
    },
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    MonoDefInstance,
    cfg::BlockID,
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    rvalue::Rvalue,
    ty::{AggregateType, MonoType},
};

use super::Builder;
use crate::context::Context;

impl Context {
    pub(super) async fn lower_call(
        &self,
        call: &IRCall,
        expression_id: IRExprID,
        block: BlockID,
        source: &IRFunction,
        builder: &mut Builder<'_>,
    ) {
        let mut arguments = call
            .arguments()
            .iter()
            .map(|argument| builder.expression_operand(*argument))
            .collect::<Vec<_>>();
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                self.apply_owner_substitution(&mut substitution);
                let (signature, effects, is_void) =
                    self.global_signature(*function_id, &substitution).await;
                let handler_position = signature
                    .parameter_types()
                    .len()
                    .checked_sub(effects.len())
                    .expect("hidden effect parameters should be part of the signature");
                for (offset, effect) in effects.into_iter().enumerate() {
                    arguments.insert(handler_position + offset, builder.handler_operand(&effect));
                }
                let callee = Operand::Function(FunctionOperand::new(
                    FunctionReference::Global(MonoDefInstance::new(*function_id, substitution)),
                    signature,
                ));
                let destination = (!is_void).then(|| builder.expression_place(expression_id));
                builder.push_instruction(
                    block,
                    Instruction::Call(Call::new(destination, callee, arguments)),
                );
                if is_void {
                    Self::assign(
                        block,
                        builder.expression_place(expression_id),
                        Rvalue::Use(Operand::Constant(Constant::Unit)),
                        builder,
                    );
                }
            }
            CallTarget::Lambda { callee } => {
                let callee_place = builder.expression_place(*callee);
                let callee_ty = self.lower_type(source.get_expression(*callee).ty()).await;
                let MonoType::Aggregate(AggregateType::Closure(closure)) = &*callee_ty else {
                    panic!("lambda callee should have a closure type")
                };
                let signature = closure.function_signature().clone();
                arguments
                    .insert(0, Operand::Copy(callee_place.clone().project_closure_environment()));
                for effect in self.lambda_effects(source.get_expression(*callee).ty()).await {
                    arguments.push(builder.handler_operand(&effect));
                }
                let code = Operand::Copy(callee_place.project_closure_function_pointer());
                let destination = builder.expression_place(expression_id);
                builder.push_instruction(
                    block,
                    Instruction::Call(Call::new(Some(destination), code, arguments)),
                );
                let _ = signature;
            }
        }
    }
}
