use rayc_ir::ir_expr::IRExprID;
use rayc_mono_ir::{
    function::LocalID,
    operand::{Constant, Operand},
    rvalue::{AddressOf, Cast, Rvalue},
    ty::{AggregateType, MonoType, PointerMutability},
};

use crate::{context::EnvironmentAbi, function_lowering::FunctionLowering};

impl FunctionLowering<'_> {
    pub(super) fn emit_opaque_environment_pointer(
        &mut self,
        args: &[IRExprID],
        environment_abi: &EnvironmentAbi,
    ) -> Operand {
        self.emit_environment(args, environment_abi).1
    }

    /// Builds an environment from `args` and returns its local together with
    /// an opaque pointer to it. An environment without captures has no local
    /// and is passed as a null pointer.
    pub(super) fn emit_environment(
        &mut self,
        args: &[IRExprID],
        environment_abi: &EnvironmentAbi,
    ) -> (Option<LocalID>, Operand) {
        assert!(!environment_abi.by_value());
        assert_eq!(args.len(), environment_abi.capture_count());
        let engine = self.engine();
        let environment = environment_abi.environment_type();
        let opaque_type = MonoType::new_opaque_pointer(engine);

        if environment.captures().is_empty() {
            return (None, Operand::Constant(Constant::NullPointer(opaque_type)));
        }

        let environment_ty =
            MonoType::new_aggregate(AggregateType::Environment(environment.clone()), engine);

        let mut fields =
            args.iter().map(|capture| self.expression_operand(*capture)).collect::<Vec<_>>();

        // Append any effect handlers embedded in this environment layout.
        fields.extend(
            environment_abi
                .captured_effects()
                .map(|effect| self.get_effect_instance_handler_operand(effect)),
        );

        // There're three steps to create the ready-to-use environment:
        //
        // 1. Initializes the fields of the environment
        // 2. Creates a pointer to the environment
        // 3. Casts the pointer to an opaque pointer type

        let environment_local = self.temporary_local(environment_ty.clone());
        let environment_place = self.place(environment_local);
        self.initialize_environment(&environment_place, fields);

        let pointer_type = MonoType::new_pointer(environment_ty, PointerMutability::Const, engine);
        let pointer = self.temporary(pointer_type);
        self.assign(
            pointer.clone(),
            Rvalue::AddressOf(AddressOf::new(environment_place, PointerMutability::Const)),
        );

        let opaque = self.temporary(opaque_type.clone());
        self.assign(opaque.clone(), Rvalue::Cast(Cast::new(Operand::Copy(pointer), opaque_type)));

        (Some(environment_local), Operand::Copy(opaque))
    }
}
