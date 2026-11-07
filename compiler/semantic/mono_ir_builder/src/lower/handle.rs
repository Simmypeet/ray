use rayc_ir::{
    cfg::{BlockID as IRBlockID, Handle as HandleTerminator, HandleClause},
    ir_expr::{IRExprKind, handle::YieldedValue},
};
use rayc_mono_ir::{
    cfg::{BlockID, Terminator},
    layout::coroutine::{RowOperationID, RowOperationKey},
    operand::Operand,
    place::FieldIndex,
};

use super::poll::{PollLoop, PollSite, Resumee, continuation_frame_field};
use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers a `handle`: resumes the continuation in a poll loop, enters the
    /// clause of each operation it handles, and forwards the others.
    pub(super) async fn lower_handle(
        &mut self,
        source_block: IRBlockID,
        terminator: &HandleTerminator,
        ctx: &Context<'_>,
    ) {
        let IRExprKind::Handle(handle) = ctx.source_expression(terminator.expression()).kind()
        else {
            panic!("a `Handle` terminator evaluates a `Handle` expression")
        };

        // Resume the continuation with the value.
        let instance =
            self.resolver().resolve_coroutine_dictionary(handle.coroutine_instance()).await;
        let resumee = Resumee::Continuation {
            continuation: ctx.expression_place(handle.continuation()),
            continuation_type: ctx.expression_type(handle.continuation()).clone(),
            value: ctx.expression_operand(handle.value()),
            value_type: ctx.expression_type(handle.value()).clone(),
        };
        let poll = self.begin_poll(PollSite::of(source_block, ctx), instance, resumee, ctx).await;

        // A completed coroutine defines the value of the `Handle` expression.
        let done = self.lower_edge(source_block, terminator.return_block(), ctx);
        let destination = ctx.expression_place(terminator.expression());
        let return_arm = self.return_arm(&poll, destination, done);

        // Each clause handles its operation.
        let mut handled = Vec::with_capacity(terminator.clauses().len());
        for (operation_id, clause) in terminator.clauses() {
            let effect = ctx.lower_handled_effect(handle, operation_id).await;
            let operation = poll.row_operation(&RowOperationKey::new(operation_id, effect));
            let arm = self.clause_arm(source_block, clause, &poll, operation, ctx).await;
            handled.push((operation, arm));
        }

        self.finish_poll(&poll, return_arm, &handled, ctx);
    }

    /// Creates the arm that enters `clause` with the arguments of `operation`
    /// and a new continuation that waits for the result of the operation.
    async fn clause_arm(
        &mut self,
        source_block: IRBlockID,
        clause: &HandleClause,
        poll: &PollLoop,
        operation: RowOperationID,
        ctx: &Context<'_>,
    ) -> BlockID {
        let edge = self.lower_edge(source_block, clause.block(), ctx);
        let arguments = self.yielded_arguments(poll, operation);

        // Each `Yielded` expression of the clause is an argument, or the
        // continuation, whose only field is the pointer to the frame that was
        // resumed.
        let mut stores = Vec::with_capacity(clause.yielded().len());
        for yielded_id in clause.yielded() {
            let IRExprKind::Yielded(value) = ctx.source_expression(*yielded_id).kind() else {
                panic!("a clause receives `Yielded` expressions")
            };
            let place = ctx.expression_place(*yielded_id);
            stores.push(match value.value() {
                YieldedValue::Argument(index) => {
                    let argument =
                        self.project(&arguments).tuple_field(FieldIndex::from_usize(index));
                    (place, Operand::Copy(argument))
                }
                YieldedValue::Continuation => {
                    let continuation_type = ctx.expression_type(*yielded_id);
                    let (field, _) =
                        continuation_frame_field(self.resolver(), continuation_type).await;
                    let frame_pointer = poll.frame_pointer().clone();
                    (self.project(&place).struct_field(field), Operand::Copy(frame_pointer))
                }
            });
        }

        self.in_new_block(|emit| {
            for (destination, operand) in stores {
                emit.store(destination, operand);
            }

            emit.set_terminator(Terminator::Goto(edge));
        })
    }
}
