use qbice::storage::intern::Interned;
use rayc_ir::{
    ir_expr::{
        IRExprID,
        call::{Call as IRCall, CallTarget},
    },
    ir_function::IRFunction,
};
use rayc_mono_ir::{
    MonoDefInstance, MonoEffectInstance, MonoIR,
    cfg::BlockID,
    instance::FunctionReference,
    instruction::{Call, Instruction},
    operand::{Constant, FunctionOperand, Operand},
    rvalue::Rvalue,
    ty::{AggregateType, FunctionSignature, MonoType, ReturnType, lower_effects, lower_type},
};
use rayc_semantic_element::{
    effect_row::get_effect_row, parameter::get_parameter_map, return_type::get_return_type,
};
use rayc_symbol::{
    GlobalSymbolID,
    symbol_kind::{SymbolKind, get_symbol_kind},
    syntax::is_variadic_def,
};
use rayc_type::{
    subst::{MutSubstitutable, Subst, Substitutable},
    ty::{Ty, application::View as ApplicationView},
};

use super::{Builder, FunctionState};

impl Builder {
    pub(super) async fn lower_call(
        &mut self,
        call: &IRCall,
        expression_id: IRExprID,
        block: BlockID,
        source: &IRFunction,
        state: &FunctionState,
        output: &mut MonoIR,
    ) {
        let mut arguments = call
            .arguments()
            .iter()
            .map(|argument| state.expression_operand(*argument))
            .collect::<Vec<_>>();
        match call.target() {
            CallTarget::Direct { function_id, subst } => {
                let mut substitution = subst.clone();
                substitution.apply_mut_subst(self.instance.substitution(), &self.engine);
                let (signature, effects, is_void) =
                    self.global_signature(*function_id, &substitution).await;
                let handler_position = signature
                    .parameter_types()
                    .len()
                    .checked_sub(effects.len())
                    .expect("hidden effect parameters should be part of the signature");
                for (offset, effect) in effects.into_iter().enumerate() {
                    arguments.insert(handler_position + offset, state.handler_operand(&effect));
                }
                let callee = Operand::Function(FunctionOperand::new(
                    FunctionReference::Global(MonoDefInstance::new(*function_id, substitution)),
                    signature,
                ));
                let destination = (!is_void).then(|| state.expression_place(expression_id));
                output.push_instruction(
                    state.target_id,
                    block,
                    Instruction::Call(Call::new(destination, callee, arguments)),
                );
                if is_void {
                    Self::assign(
                        block,
                        state.expression_place(expression_id),
                        Rvalue::Use(Operand::Constant(Constant::Unit)),
                        state,
                        output,
                    );
                }
            }
            CallTarget::Lambda { callee } => {
                let callee_place = state.expression_place(*callee);
                let callee_ty = self.lower_type(source.get_expression(*callee).ty()).await;
                let MonoType::Aggregate(AggregateType::Closure(closure)) = &*callee_ty else {
                    panic!("lambda callee should have a closure type")
                };
                let signature = closure.function_signature().clone();
                arguments
                    .insert(0, Operand::Copy(callee_place.clone().project_closure_environment()));
                for effect in self.lambda_effects(source.get_expression(*callee).ty()).await {
                    arguments.push(state.handler_operand(&effect));
                }
                let code = Operand::Copy(callee_place.project_closure_function_pointer());
                output.push_instruction(
                    state.target_id,
                    block,
                    Instruction::Call(Call::new(
                        Some(state.expression_place(expression_id)),
                        code,
                        arguments,
                    )),
                );
                let _ = signature;
            }
        }
    }

    async fn global_signature(
        &mut self,
        function_id: GlobalSymbolID,
        substitution: &Subst,
    ) -> (FunctionSignature, Vec<MonoEffectInstance>, bool) {
        let parameters = self.engine.get_parameter_map(function_id).await;
        let mut parameter_types = Vec::new();
        for (_, parameter) in parameters.iter() {
            parameter_types.push(self.engine.lower_type(parameter.ty(), substitution).await);
        }
        let symbol_kind = self.engine.get_symbol_kind(function_id).await;
        let effects = if symbol_kind == SymbolKind::Def {
            let effect = self.engine.get_effect_row(function_id).await;
            self.engine.lower_effects(&effect, substitution).await
        } else {
            Vec::new()
        };
        for effect in &effects {
            parameter_types.push(MonoType::new_handler_pointer(effect.clone(), &self.engine));
        }
        let return_type = self.engine.get_return_type(function_id).await;
        let return_type = self.engine.lower_type(&return_type, substitution).await;
        let is_void =
            symbol_kind == SymbolKind::ExternDef && matches!(&*return_type, MonoType::Unit);
        let return_type = if is_void {
            ReturnType::Void
        } else {
            ReturnType::Value(self.engine.intern_unsized([return_type]))
        };
        let is_variadic = if matches!(symbol_kind, SymbolKind::Def | SymbolKind::ExternDef) {
            self.engine.is_variadic_def(function_id).await
        } else {
            false
        };
        let parameter_types = self.engine.intern_unsized(parameter_types);
        let signature = if is_variadic {
            FunctionSignature::new_variadic(parameter_types, return_type)
        } else {
            FunctionSignature::new(parameter_types, return_type)
        };
        (signature, effects, is_void)
    }

    async fn lambda_effects(&mut self, ty: &Interned<Ty>) -> Vec<MonoEffectInstance> {
        let ty = ty.apply_subst_or_clone(self.instance.substitution(), &self.engine);
        let Ty::Application(application) = &*ty else {
            panic!("lambda callee should have an application type")
        };
        let ApplicationView::Lambda(lambda) = application.view() else {
            panic!("lambda callee should have a lambda type")
        };
        self.engine.lower_effects(lambda.effect_row(), &Subst::new_empty()).await
    }
}
