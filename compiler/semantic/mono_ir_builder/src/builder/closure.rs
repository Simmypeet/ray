use rayc_ir::ir_expr::{IRExprID, handle::HandledFunction, make_lambda::MakeLambda};
use rayc_mono_ir::{
    cfg::BlockID,
    function::{Local, LocalKind},
    instance::FunctionReference,
    operand::{Constant, FunctionOperand, Operand},
    place::Place,
    rvalue::{AddressOf, AggregateValue, Cast, Rvalue},
    ty::{AggregateType, MonoType, PointerMutability},
};

use super::Builder;
use crate::{context::Context, function_abi::FunctionABI};

impl Context {
    pub(super) fn lower_make_lambda(
        &self,
        lambda: &MakeLambda,
        expression_id: IRExprID,
        block: BlockID,
        builder: &mut Builder<'_>,
    ) {
        let abi = self.function_abi(lambda.function_id());
        let handled = HandledFunction::new(lambda.function_id(), lambda.captures().to_vec());
        let environment = self.emit_environment(&handled, &abi, block, builder);
        let closure_type = self.create_closure_type(abi.signature().clone());
        Self::assign(
            block,
            builder.expression_place(expression_id),
            Rvalue::Aggregate(AggregateValue::new(closure_type, vec![
                environment,
                Operand::Function(FunctionOperand::new(
                    FunctionReference::Local(builder.target_id()),
                    abi.signature().clone(),
                )),
            ])),
            builder,
        );
    }

    pub(super) fn emit_environment(
        &self,
        handled: &HandledFunction,
        abi: &FunctionABI,
        block: BlockID,
        builder: &mut Builder<'_>,
    ) -> Operand {
        assert_eq!(handled.captures().len(), abi.capture_count());
        let environment_type = abi.environment_type();
        let MonoType::Aggregate(AggregateType::Environment(environment)) = &*environment_type
        else {
            panic!("nested function environment should be an aggregate")
        };
        if environment.captures().is_empty() {
            return Operand::Constant(Constant::NullPointer(self.create_opaque_pointer()));
        }

        let mut fields = handled
            .captures()
            .iter()
            .map(|capture| builder.expression_operand(*capture))
            .collect::<Vec<_>>();
        if abi.captures_effect_handlers() {
            fields.extend(abi.effects().map(|effect| builder.handler_operand(effect)));
        }
        let environment_local =
            builder.insert_local(Local::new(environment_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(environment_local),
            Rvalue::Aggregate(AggregateValue::new(environment_type.clone(), fields)),
            builder,
        );
        let pointer_type = self.create_pointer(environment_type, PointerMutability::Const);
        let pointer_local = builder.insert_local(Local::new(pointer_type, LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(pointer_local),
            Rvalue::AddressOf(AddressOf::new(
                Place::new(environment_local),
                PointerMutability::Const,
            )),
            builder,
        );
        let opaque_type = self.create_opaque_pointer();
        let opaque_local =
            builder.insert_local(Local::new(opaque_type.clone(), LocalKind::Temporary));
        Self::assign(
            block,
            Place::new(opaque_local),
            Rvalue::Cast(Cast::new(Operand::Copy(Place::new(pointer_local)), opaque_type)),
            builder,
        );
        Operand::Copy(Place::new(opaque_local))
    }
}
