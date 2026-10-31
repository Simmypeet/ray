use std::collections::BTreeMap;

use rayc_ir::{
    cfg::{BlockID as IRBlockID, Handle as HandleTerminator, HandleClause},
    ir_expr::{
        IRExprKind,
        handle::{Handle, YieldedValue},
    },
};
use rayc_mono_ir::{
    MonoCoroutineInstance,
    cfg::{BlockID, Switch, Terminator},
    instance::FunctionReference,
    layout::EffectRowLayout,
    operand::Operand,
    place::{FieldIndex, Place},
    rvalue::Rvalue,
    signature::CoroutineSignature,
    ty::{AggregateType, CoroutineProgress, CoroutineResume, MonoType},
};
use rayc_semantic_element::struct_body::FieldID;

use crate::{function_lowering::FunctionLowering, resolver::Resolver};

/// What the edges out of a `handle` read.
struct ResumedCoroutine<'a> {
    /// The local that holds what the resume returned.
    progress: &'a Place,
    /// The frame that was resumed, which a clause's continuation keeps.
    frame_pointer: &'a Operand,
    /// The field of `core.Continuation` that holds the frame pointer.
    continuation_field: FieldID,
    row_layout: &'a EffectRowLayout,
}

/// The field of a `core.Continuation`, which holds the frame pointer.
async fn continuation_field(resolver: &Resolver, continuation: &MonoType) -> FieldID {
    let layout = resolver.struct_layout(continuation.assert_as_struct()).await;
    *layout.fields().keys().next().expect("`core.Continuation` should have a field")
}

impl FunctionLowering<'_> {
    /// Lowers a `handle`: resumes the coroutine, and branches on how it came
    /// back.
    pub(super) async fn lower_handle(
        &mut self,
        source_block: IRBlockID,
        terminator: &HandleTerminator,
    ) {
        let IRExprKind::Handle(handle) = self.source_expression(terminator.expression()).kind()
        else {
            panic!("a `Handle` terminator evaluates a `Handle` expression")
        };
        let resolver = self.resolver();

        // The row is checked here rather than through the abandon block, which
        // was decided before the substitution could empty the row.
        assert!(
            resolver.lower_effects(handle.forwarded_effect()).await.is_empty(),
            "forwarding the operations of a resumed coroutine is not lowered to MonoIR yet"
        );

        let instance = resolver.resolve_coroutine_dictionary(handle.coroutine_instance()).await;
        let signature = resolver.coroutine_signature(&instance).await;
        let row_layout = resolver.effect_row_layout(signature.row()).await;

        let continuation = self.expression_place(handle.continuation());
        let field = continuation_field(resolver, self.expression_type(handle.continuation())).await;
        let frame_pointer = Operand::Copy(self.project(&continuation).struct_field(field));

        let progress = self.lower_resume_call(
            handle,
            &instance,
            &signature,
            &row_layout,
            frame_pointer.clone(),
        );

        // Lower the edges while this block is still open, then branch to them.
        let return_block = self.lower_handle_return_edge(source_block, terminator, &progress);
        let resumed = ResumedCoroutine {
            progress: &progress,
            frame_pointer: &frame_pointer,
            continuation_field: field,
            row_layout: &row_layout,
        };
        let yield_block =
            self.lower_handle_yield_edges(source_block, terminator, handle, &resumed).await;

        let tag = self.project(&progress).coroutine_progress_tag();
        self.set_switch_terminator(Operand::Copy(tag), vec![
            (CoroutineProgress::return_tag(), return_block),
            (CoroutineProgress::yield_tag(), yield_block),
        ]);
    }

    /// Calls `coroutine_resume` with the value of `handle`, and returns the
    /// place of the progress.
    fn lower_resume_call(
        &mut self,
        handle: &Handle,
        instance: &MonoCoroutineInstance,
        signature: &CoroutineSignature,
        row_layout: &EffectRowLayout,
        frame_pointer: Operand,
    ) -> Place {
        // `args` holds the value in the member of its type.
        let args = self.temporary(MonoType::new_aggregate(
            AggregateType::CoroutineResume(CoroutineResume::new(signature.row().clone())),
            self.engine(),
        ));
        let value = row_layout.resume_index(self.expression_type(handle.value()));
        self.store(
            self.project(&args).coroutine_resume_value(value),
            self.expression_operand(handle.value()),
        );

        let progress = self.progress_temporary(signature);
        self.call(
            Some(progress.clone()),
            Operand::Function(FunctionReference::CoroutineResume(instance.clone())),
            vec![frame_pointer, Operand::Copy(args)],
        );
        progress
    }

    /// Creates the block taken when the coroutine completed, which defines the
    /// value of the `Handle` expression.
    fn lower_handle_return_edge(
        &mut self,
        source_block: IRBlockID,
        terminator: &HandleTerminator,
        progress: &Place,
    ) -> BlockID {
        let edge = self.lower_edge(source_block, terminator.return_block());
        let destination = self.expression_place(terminator.expression());
        let value = self.project(progress).coroutine_progress_return();

        self.in_new_block(|emit| {
            emit.store(destination, Operand::Copy(value));
            emit.set_terminator(Terminator::Goto(edge));
        })
    }

    /// Creates the block taken when the coroutine suspended, which branches
    /// to the clause of its operation.
    async fn lower_handle_yield_edges(
        &mut self,
        source_block: IRBlockID,
        terminator: &HandleTerminator,
        handle: &Handle,
        resumed: &ResumedCoroutine<'_>,
    ) -> BlockID {
        let yielded = self.project(resumed.progress).coroutine_progress_yield();

        // A map orders the clauses by tag, whatever order they are stored in.
        let mut targets = Vec::with_capacity(terminator.clauses().len());
        for (operation_id, clause) in terminator.clauses() {
            let effect = self.lower_handled_effect(handle, operation_id).await;
            let operation = resumed.row_layout.operation_index(&effect, operation_id);

            let arguments = self.project(&yielded).coroutine_yield_operation(operation);
            let block = self.lower_clause_edge(source_block, clause, &arguments, resumed).await;
            targets.push((resumed.row_layout.yield_tag(operation), block));
        }

        let tag = self.project(&yielded).coroutine_yield_tag();
        let otherwise = self.in_new_block(|emit| emit.set_terminator(Terminator::Unreachable));

        self.in_new_block(|emit| {
            emit.set_terminator(Terminator::Switch(Switch::new(
                Operand::Copy(tag),
                targets,
                otherwise,
            )));
        })
    }

    /// Creates the block that enters `clause`, which receives the arguments at
    /// `arguments` and a new continuation that waits for the result of the
    /// operation.
    async fn lower_clause_edge(
        &mut self,
        source_block: IRBlockID,
        clause: &HandleClause,
        arguments: &Place,
        resumed: &ResumedCoroutine<'_>,
    ) -> BlockID {
        let edge = self.lower_edge(source_block, clause.block());

        let mut values = Vec::with_capacity(clause.yielded().len());
        for yielded_id in clause.yielded() {
            let IRExprKind::Yielded(value) = self.source_expression(*yielded_id).kind() else {
                panic!("a clause receives `Yielded` expressions")
            };
            let value = match value.value() {
                YieldedValue::Argument(index) => Rvalue::Use(Operand::Copy(
                    self.project(arguments).tuple_field(FieldIndex::from_usize(index)),
                )),
                YieldedValue::Continuation => {
                    let ty = self.expression_type(*yielded_id).assert_as_struct();
                    let layout = self.resolver().struct_layout(ty).await;
                    Rvalue::new_struct(
                        &layout,
                        BTreeMap::from([(
                            resumed.continuation_field,
                            resumed.frame_pointer.clone(),
                        )]),
                    )
                }
            };

            values.push((self.expression_place(*yielded_id), value));
        }

        self.in_new_block(|emit| {
            for (destination, value) in values {
                emit.assign(destination, value);
            }

            emit.set_terminator(Terminator::Goto(edge));
        })
    }
}
