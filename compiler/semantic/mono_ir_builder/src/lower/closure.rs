use rayc_ir::ir_expr::IRExprID;
use rayc_mono_ir::{
    function::{Local, LocalKind},
    operand::{Constant, Operand},
    place::Place,
    rvalue::{AddressOf, Cast, Rvalue},
    ty::PointerMutability,
};

use crate::{builder::Builder, context::Context, function_abi::EnvironmentABI};

impl Builder<'_> {
    pub(super) fn lower_closure(
        &mut self,
        context: &Context,
        lambda: &rayc_ir::ir_expr::closure::Closure,
        expression_id: IRExprID,
    ) {
        // Captures have already been evaluated in semantic IR order.
        let environment_abi = context.function_environment_abi(lambda.function_id());
        let environment = environment_abi.environment_type();
        let fields =
            lambda.captures().iter().map(|capture| self.expression_operand(*capture)).collect();

        // Effect handlers are passed as additional arguments to the function, not
        // embedded in the environment.
        assert_eq!(environment_abi.captured_effects().len(), 0);

        self.assign(
            self.expression_place(expression_id),
            Rvalue::new_environment(environment.clone(), fields),
        );
    }

    pub(super) fn emit_opaque_environment_pointer(
        &mut self,
        context: &Context,
        args: &[IRExprID],
        environment_abi: &EnvironmentABI,
    ) -> Operand {
        assert!(!environment_abi.by_value());
        assert_eq!(args.len(), environment_abi.capture_count());
        let environment = environment_abi.environment_type();

        if environment.captures().is_empty() {
            return Operand::Constant(Constant::NullPointer(context.create_opaque_pointer()));
        }

        let environment_ty = context.intern_environment_type(environment.clone());

        let mut fields =
            args.iter().map(|capture| self.expression_operand(*capture)).collect::<Vec<_>>();

        // Append any effect handlers embedded in this environment layout.
        fields
            .extend(environment_abi.captured_effects().map(|effect| self.handler_operand(effect)));

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
