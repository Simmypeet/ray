use rayc_ir::ir_closure::CaptureOperands;
use rayc_mono_ir::{
    function::LocalID,
    layout::EnvironmentLayout,
    operand::{Constant, Operand},
    rvalue::{AddressOf, Cast, Rvalue},
    ty::{MonoType, PointerMutability},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    pub(super) fn emit_opaque_environment_pointer(
        &mut self,
        args: &CaptureOperands,
        layout: &EnvironmentLayout,
        ctx: &Context<'_>,
    ) -> Operand {
        self.emit_environment(args, layout, ctx).1
    }

    /// Builds the environment with `layout` from `args`, the operand of each of
    /// its captures, and returns its local together with an opaque pointer to
    /// it. An environment that holds nothing has no local and is passed as a
    /// null pointer.
    ///
    /// Only an environment passed behind a pointer is built this way: a
    /// closure's is its value.
    pub(super) fn emit_environment(
        &mut self,
        args: &CaptureOperands,
        layout: &EnvironmentLayout,
        ctx: &Context<'_>,
    ) -> (Option<LocalID>, Operand) {
        assert_eq!(args.len(), layout.capture_ids().len());
        let engine = self.engine();
        let opaque_type = MonoType::new_opaque_pointer(engine);

        if layout.is_empty() {
            return (None, Operand::Constant(Constant::NullPointer(opaque_type)));
        }

        // The captures, then the handlers that an operation handler performs
        // with, taken from this function's own.
        let captures = layout
            .capture_ids()
            .map(|capture| (capture, ctx.expression_operand(args.get_operand_of(capture))));
        let handler_records = layout.handler_records().map(|(record_id, record)| {
            (record_id, ctx.get_effect_instance_handler_operand(record.effect()))
        });

        // There're three steps to create the ready-to-use environment:
        //
        // 1. Initializes the fields of the environment
        // 2. Creates a pointer to the environment
        // 3. Casts the pointer to an opaque pointer type

        let environment_ty = engine.intern(layout.environment_type());
        let environment_local = self.new_temporary_local(environment_ty.clone());
        let environment_place = self.local_place(environment_local);
        self.initialize_environment(&environment_place, captures, handler_records);

        let pointer_type = MonoType::new_pointer(environment_ty, PointerMutability::Const, engine);
        let pointer = self.new_temporary_place(pointer_type);
        self.assign(
            pointer.clone(),
            Rvalue::AddressOf(AddressOf::new(environment_place, PointerMutability::Const)),
        );

        let opaque = self.new_temporary_place(opaque_type.clone());
        self.assign(opaque.clone(), Rvalue::Cast(Cast::new(Operand::Copy(pointer), opaque_type)));

        (Some(environment_local), Operand::Copy(opaque))
    }
}
