use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{
    cfg::Suspend,
    ir_expr::{IRExprID, IRExprKind, perform::Perform},
};
use rayc_mono_ir::{
    MonoEffectInstance,
    cfg::{BlockID, Terminator},
    layout::coroutine::{FrameState, RowOperationKey},
    operand::Operand,
    place::Place,
    ty::CoroutineProgress,
};

use crate::function_lowering::FunctionLowering;

impl FunctionLowering<'_> {
    /// Lowers a `perform` of a coroutine: records where to continue, and
    /// returns the operation to whatever resumed the coroutine.
    pub(super) async fn lower_yield(&mut self, point: SuspensionPointID, suspend: &Suspend) {
        let IRExprKind::Perform(perform) = self.source_expression(suspend.expression()).kind()
        else {
            panic!("a `perform` should be what this suspension point evaluates")
        };

        // Control comes back through a block that defines the result of the
        // `perform` and continues on the resume edge.
        let original_target = self.lower_edge(point.block_id(), suspend.resume_block());
        self.create_resume_block(point, suspend.expression(), original_target);

        let effect_instance =
            self.instantiate_effect(perform.effect_id(), perform.substitution()).await;

        let coroutine_form = self.coroutine_form();
        let coroutine_state_place =
            self.project(coroutine_form.frame_place()).coroutine_frame_state();
        let suspended_state = coroutine_form.frame_state_tag(FrameState::Suspended(point));

        self.store_constant(coroutine_state_place, suspended_state);

        let progress = self.create_progress_yield(effect_instance, perform);

        self.set_terminator(Terminator::Return(Some(progress)));
    }

    fn create_progress_yield(
        &mut self,
        effect_instance: MonoEffectInstance,
        perform: &Perform,
    ) -> Operand {
        let coroutine_form = self.coroutine_form();
        let key = RowOperationKey::new(perform.operation_id(), effect_instance);
        let operation = coroutine_form.row_layout().get_row_operation_id(&key);
        let yield_tag = coroutine_form.row_layout().yield_tag(operation);

        let progress = coroutine_form.progress_place().clone();

        self.store_constant(
            self.project(&progress).coroutine_progress_tag(),
            CoroutineProgress::yield_tag(),
        );

        let yielded = self.project(&progress).coroutine_progress_yield_payload();

        self.store_constant(self.project(&yielded).coroutine_yield_tag(), yield_tag);

        let arguments = self.project(&yielded).coroutine_yield_operation(operation);
        self.initialize_yield_arguments(&arguments, perform);

        Operand::Copy(progress)
    }

    /// Creates the block `coroutine_resume` dispatches to for `point`.
    fn create_resume_block(
        &mut self,
        point: SuspensionPointID,
        expression: IRExprID,
        original_target: BlockID,
    ) {
        let form = self.coroutine_form();
        let value = form.row_layout().get_resume_variant_id(self.expression_type(expression));
        let args = self.project(form.resume_args_place()).coroutine_resume_value(value);
        let dispatched_resume = form.dispatched_resume_block(point);
        let destination = self.expression_place(expression);

        self.in_block(dispatched_resume, |emit| {
            emit.store(destination, Operand::Copy(args));
            emit.set_terminator(Terminator::Goto(original_target));
        });
    }

    fn initialize_yield_arguments(&mut self, destination: &Place, perform: &Perform) {
        let fields =
            perform.arguments().iter().map(|argument| self.expression_operand(*argument)).collect();

        self.initialize_tuple(destination, fields);
    }
}
