use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{cfg::Suspend, ir_expr::IRExprKind};
use rayc_mono_ir::{cfg::Terminator, layout::coroutine::RowOperationKey, operand::Operand};

use super::YieldArguments;
use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers a `perform` of a coroutine: records where to continue, and
    /// returns the operation to whatever resumed the coroutine.
    pub(in crate::lower) async fn lower_yield_perform(
        &mut self,
        point: SuspensionPointID,
        suspend: &Suspend,
        ctx: &Context<'_>,
    ) {
        let IRExprKind::Perform(perform) = ctx.source_expression(suspend.expression()).kind()
        else {
            panic!("a `perform` should be what this suspension point evaluates")
        };

        // Once resumed, the result of the `perform` is in the member of `args`
        // of its type, and control continues on the resume edge.
        let after = self.lower_edge(point.block_id(), suspend.resume_block(), ctx);
        let destination = ctx.expression_place(suspend.expression());
        let form = ctx.coroutine_form();
        let member =
            form.row_layout().get_resume_variant_id(ctx.expression_type(suspend.expression()));
        let result = self.project(form.resume_args_place()).coroutine_resume_value(member);
        self.fill_resume_entry(point, ctx, |emit| {
            emit.store(destination, Operand::Copy(result));
            emit.set_terminator(Terminator::Goto(after));
        });

        // Suspend here, and yield the operation with its arguments.
        let effect = ctx.instantiate_effect(perform.effect_id(), perform.substitution()).await;
        let own_yield =
            self.create_own_yield(&RowOperationKey::new(perform.operation_id(), effect), ctx);
        let arguments =
            perform.arguments().iter().map(|argument| ctx.expression_operand(*argument)).collect();

        self.store_suspended_state(point, ctx);
        self.emit_own_yield(own_yield, YieldArguments::Elements(arguments));
    }
}
