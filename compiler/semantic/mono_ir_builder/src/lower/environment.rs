use rayc_ir::ir_closure::CaptureOperands;
use rayc_mono_ir::{
    function::LocalID,
    layout::EnvironmentLayout,
    operand::{Constant, Operand},
    rvalue::{AddressOf, Cast, Rvalue},
    ty::{AggregateType, MonoType, PointerMutability},
};

use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Builds the environment with `layout` from `args`, the operand of each of
    /// its captures, and returns it, for a callee that receives it by value:
    ///
    /// ```c
    /// environment_t environment = { capture_0, ..., handler_F };
    /// ```
    ///
    /// An environment that holds nothing is passed as its empty value.
    pub(super) fn emit_environment_value(
        &mut self,
        args: &CaptureOperands,
        layout: &EnvironmentLayout,
        ctx: &Context<'_>,
    ) -> Operand {
        self.build_environment(args, layout, ctx).map_or_else(
            || {
                let environment = AggregateType::Environment(layout.instance().clone());
                Operand::Constant(Constant::EmptyAggregate(environment))
            },
            |environment| Operand::Copy(self.local_place(environment)),
        )
    }

    /// Builds the environment with `layout` from `args`, the operand of each of
    /// its captures, and returns its local together with an opaque pointer to
    /// it. An environment that holds nothing has no local and is passed as a
    /// null pointer.
    ///
    /// Only an environment passed behind a pointer is built this way: the
    /// clauses of a `run ... with` share theirs.
    pub(super) fn emit_environment(
        &mut self,
        args: &CaptureOperands,
        layout: &EnvironmentLayout,
        ctx: &Context<'_>,
    ) -> (Option<LocalID>, Operand) {
        let engine = self.engine();
        let opaque_type = MonoType::new_opaque_pointer(engine);

        let Some(environment_local) = self.build_environment(args, layout, ctx) else {
            return (None, Operand::Constant(Constant::NullPointer(opaque_type)));
        };

        // The environment is passed through a pointer to it, cast to an opaque
        // pointer type.
        let environment_ty = engine.intern(layout.environment_type());
        let pointer_type = MonoType::new_pointer(environment_ty, PointerMutability::Const, engine);
        let pointer = self.new_temporary_place(pointer_type);
        self.assign(
            pointer.clone(),
            Rvalue::AddressOf(AddressOf::new(
                self.local_place(environment_local),
                PointerMutability::Const,
            )),
        );

        let opaque = self.new_temporary_place(opaque_type.clone());
        self.assign(opaque.clone(), Rvalue::Cast(Cast::new(Operand::Copy(pointer), opaque_type)));

        (Some(environment_local), Operand::Copy(opaque))
    }

    /// Initializes a new local of the environment with `layout` from `args`,
    /// the operand of each of its captures, and from this function's handler
    /// of each of its handler records. An environment that holds nothing has
    /// no local.
    fn build_environment(
        &mut self,
        args: &CaptureOperands,
        layout: &EnvironmentLayout,
        ctx: &Context<'_>,
    ) -> Option<LocalID> {
        assert_eq!(args.len(), layout.capture_ids().len());
        if layout.is_empty() {
            return None;
        }

        // The captures, then the handlers that an operation handler performs
        // with, taken from this function's own.
        let captures = layout
            .capture_ids()
            .map(|capture| (capture, ctx.expression_operand(args.get_operand_of(capture))));
        let handler_records = layout.handler_records().map(|(record_id, record)| {
            (record_id, ctx.get_effect_instance_handler_operand(record.effect()))
        });

        let environment_ty = self.engine().intern(layout.environment_type());
        let environment_local = self.new_temporary_local(environment_ty);
        let environment_place = self.local_place(environment_local);
        self.initialize_environment(&environment_place, captures, handler_records);

        Some(environment_local)
    }
}
