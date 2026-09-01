use rayc_ir::ir_expr::{IRExprID, handle::HandledFunction, make_lambda::MakeLambda};
use rayc_mono_ir::{
    cfg::BlockID,
    function::{Local, LocalKind},
    instance::FunctionReference,
    operand::{Constant, FunctionOperand, Operand},
    place::Place,
    rvalue::{AddressOf, AggregateValue, Cast, Rvalue},
    ty::{AggregateType, Closure, FunctionSignature, MonoType, PointerMutability},
};

use super::{Builder, BuilderState};
use crate::function_abi::FunctionABI;

impl Builder {
    pub(super) fn lower_make_lambda(
        &mut self,
        lambda: &MakeLambda,
        expression_id: IRExprID,
        block: BlockID,
        builder_state: &mut BuilderState<'_>,
    ) {
        let abi = self.function_abi(lambda.function_id()).clone();
        let handled = HandledFunction::new(lambda.function_id(), lambda.captures().to_vec());
        let environment = self.emit_environment(&handled, &abi, block, builder_state);
        let closure_type = self.closure_type(abi.signature().clone());
        Self::assign(
            block,
            builder_state.expression_place(expression_id),
            Rvalue::Aggregate(AggregateValue::new(closure_type, vec![
                environment,
                Operand::Function(FunctionOperand::new(
                    FunctionReference::Local(builder_state.target_id()),
                    abi.signature().clone(),
                )),
            ])),
            builder_state,
        );
    }

    pub(super) fn emit_environment(
        &mut self,
        handled: &HandledFunction,
        abi: &FunctionABI,
        block: BlockID,
        builder_state: &mut BuilderState<'_>,
    ) -> Operand {
        assert_eq!(handled.captures().len(), abi.capture_count());
        let environment_type = abi.environment_type();
        let MonoType::Aggregate(AggregateType::Environment(environment)) = &*environment_type
        else {
            panic!("nested function environment should be an aggregate")
        };
        if environment.captures().is_empty() {
            return Operand::Constant(Constant::NullPointer(MonoType::new_opaque_pointer(
                &self.engine,
            )));
        }

        let mut fields = handled
            .captures()
            .iter()
            .map(|capture| builder_state.expression_operand(*capture))
            .collect::<Vec<_>>();
        if abi.captures_effect_handlers() {
            fields.extend(abi.effects().map(|effect| builder_state.handler_operand(effect)));
        }
        let environment_local =
            builder_state.insert_local(Local::new(environment_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(environment_local),
            Rvalue::Aggregate(AggregateValue::new(environment_type.clone(), fields)),
            builder_state,
        );
        let pointer_type =
            MonoType::new_pointer(environment_type, PointerMutability::Const, &self.engine);
        let pointer_local =
            builder_state.insert_local(Local::new(pointer_type, LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(pointer_local),
            Rvalue::AddressOf(AddressOf::new(
                Place::new(environment_local),
                PointerMutability::Const,
            )),
            builder_state,
        );
        let opaque_type = MonoType::new_opaque_pointer(&self.engine);
        let opaque_local =
            builder_state.insert_local(Local::new(opaque_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(opaque_local),
            Rvalue::Cast(Cast::new(Operand::Copy(Place::new(pointer_local)), opaque_type)),
            builder_state,
        );
        Operand::Copy(Place::new(opaque_local))
    }

    pub(super) fn closure_type(
        &self,
        signature: FunctionSignature,
    ) -> qbice::storage::intern::Interned<MonoType> {
        self.engine.intern(MonoType::Aggregate(AggregateType::Closure(Closure::new(signature))))
    }
}
