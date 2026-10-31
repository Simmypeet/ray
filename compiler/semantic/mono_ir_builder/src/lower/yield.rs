use rayc_ir::{
    cfg::{BlockID as IRBlockID, Suspend},
    ir_expr::{IRExprID, IRExprKind, perform::Perform},
};
use rayc_mono_ir::{
    MonoEffectInstance,
    cfg::{BlockID, Terminator},
    operand::Operand,
    place::FieldIndex,
    rvalue::Rvalue,
    ty::CoroutineProgress,
};

use crate::{function_lowering::FunctionLowering, lower::expression::tuple_value};

impl FunctionLowering<'_> {
    /// Lowers a `perform` of a coroutine: records where to continue, and
    /// returns the operation to whatever resumed the coroutine.
    pub(super) fn lower_yield(
        &mut self,
        source_block: IRBlockID,
        suspend: &Suspend,
        variant: FieldIndex,
    ) {
        let expression = self.source_expression(suspend.expression());
        let IRExprKind::Perform(perform) = expression.kind() else {
            panic!("only a `perform` of a coroutine suspends, until forwarding is lowered")
        };

        // Control comes back through a block that defines the result of the
        // `perform` and continues on the resume edge.
        let original_target = self.lower_edge(source_block, suspend.resume_block());
        self.create_resume_block(variant, suspend.expression(), original_target);

        let effect_instance = MonoEffectInstance::of_perform(perform);

        let coroutine_form = self.coroutine_form();
        let coroutine_state_place =
            self.project(coroutine_form.frame_place()).coroutine_frame_state();
        let suspended_state = coroutine_form.frame().layout().suspended_state(variant);

        self.store_constant(coroutine_state_place, suspended_state);

        let progress = self.create_progress_yield(&effect_instance, perform);

        self.set_terminator(Terminator::Return(Some(progress)));
    }

    fn create_progress_yield(
        &mut self,
        effect_instance: &MonoEffectInstance,
        perform: &Perform,
    ) -> Operand {
        let coroutine_form = self.coroutine_form();
        let operation =
            coroutine_form.row_layout().operation_index(effect_instance, perform.operation_id());
        let yield_tag = coroutine_form.row_layout().yield_tag(operation);

        let progress = coroutine_form.progress_place().clone();

        self.store_constant(
            self.project(&progress).coroutine_progress_tag(),
            CoroutineProgress::yield_tag(),
        );

        let yielded = self.project(&progress).coroutine_progress_yield();

        let arguments = self.yield_arguments(perform, operation);

        self.store_constant(self.project(&yielded).coroutine_yield_tag(), yield_tag);
        self.assign(self.project(&yielded).coroutine_yield_operation(operation), arguments);

        Operand::Copy(progress)
    }

    /// Creates the block `coroutine_resume` dispatches to for `variant`.
    fn create_resume_block(
        &mut self,
        variant: FieldIndex,
        expression: IRExprID,
        original_target: BlockID,
    ) {
        let form = self.coroutine_form();
        let value = form.row_layout().resume_index(self.expression_type(expression));
        let args = self.project(form.resume_args_place()).coroutine_resume_value(value);
        let dispatched_resume = form.dispatched_resume_block(variant);
        let destination = self.expression_place(expression);

        self.in_block(dispatched_resume, |emit| {
            emit.store(destination, Operand::Copy(args));
            emit.set_terminator(Terminator::Goto(original_target));
        });
    }

    fn yield_arguments(&self, perform: &Perform, operation: FieldIndex) -> Rvalue {
        let form = self.coroutine_form();
        let ty = form.row_layout().operations()[operation.index() as usize].arguments();
        let fields =
            perform.arguments().iter().map(|argument| self.expression_operand(*argument)).collect();

        tuple_value(ty.assert_as_tuple(), fields)
    }
}
