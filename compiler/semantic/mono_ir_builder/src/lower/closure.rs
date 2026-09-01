use rayc_ir::ir_expr::{IRExprID, make_lambda::MakeLambda};
use rayc_mono_ir::{
    function::{Local, LocalKind},
    instance::FunctionReference,
    operand::{Constant, FunctionOperand, Operand},
    place::Place,
    rvalue::{AddressOf, Cast, Rvalue},
    ty::{Closure, PointerMutability},
};

use crate::{builder::Builder, context::Context, function_abi::FunctionABI};

impl Builder<'_> {
    pub(super) fn lower_make_lambda(
        &mut self,
        context: &Context,
        lambda: &MakeLambda,
        expression_id: IRExprID,
    ) {
        let abi = context.function_abi(lambda.function_id());
        let environment = self.emit_environment(context, lambda.captures(), abi);

        let function_ref = Operand::Function(FunctionOperand::new(
            FunctionReference::Local(context.target_function_id(lambda.function_id())),
            abi.signature().clone(),
        ));

        self.assign(
            self.expression_place(expression_id),
            Rvalue::new_closure(Closure::new(abi.signature().clone()), environment, function_ref),
        );
    }

    pub(super) fn emit_environment(
        &mut self,
        context: &Context,
        args: &[IRExprID],
        abi: &FunctionABI,
    ) -> Operand {
        assert_eq!(args.len(), abi.capture_count());
        let environment = abi.environment_type();

        if environment.captures().is_empty() {
            return Operand::Constant(Constant::NullPointer(context.create_opaque_pointer()));
        }

        let environment_ty = context.intern_environment_type(environment.clone());

        let mut fields =
            args.iter().map(|capture| self.expression_operand(*capture)).collect::<Vec<_>>();

        // determine whether the effect handlers are embedded in the environment and if
        // so, add them to the fields
        if abi.captures_effect_handlers() {
            fields.extend(abi.effects().map(|effect| self.handler_operand(effect)));
        }

        // There're three steps to create the ready-to-use environment:
        //
        // 1. Constructs the environment aggregate value
        // 2. Creates a pointer to the environment aggregate value
        // 3. Casts the pointer to an opaque pointer type

        let environment_local =
            self.insert_local(Local::new(environment_ty.clone(), LocalKind::Temporary));
        self.assign(
            Place::new(environment_local),
            Rvalue::new_environment(environment.clone(), fields),
        );

        let pointer_type = context.create_pointer(environment_ty, PointerMutability::Const);
        let pointer_local = self.insert_local(Local::new(pointer_type, LocalKind::Temporary));
        self.assign(
            Place::new(pointer_local),
            Rvalue::AddressOf(AddressOf::new(
                Place::new(environment_local),
                PointerMutability::Const,
            )),
        );

        let opaque_type = context.create_opaque_pointer();
        let opaque_local = self.insert_local(Local::new(opaque_type.clone(), LocalKind::Temporary));
        self.assign(
            Place::new(opaque_local),
            Rvalue::Cast(Cast::new(Operand::Copy(Place::new(pointer_local)), opaque_type)),
        );

        Operand::Copy(Place::new(opaque_local))
    }
}
