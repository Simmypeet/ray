use qbice::storage::intern::Interned;
use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::ir_expr::call::Call as IRCall;
use rayc_mono_ir::{
    MonoCoroutineInstance,
    cfg::BlockID,
    operand::Operand,
    place::{FieldIndex, Place},
    ty::MonoType,
};

use super::poll::{PollSite, Resumee};
use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// The coroutine `call` resumes, if it calls a continuation.
    pub(super) async fn resumed_continuation(
        &self,
        call: &IRCall,
    ) -> Option<MonoCoroutineInstance> {
        let dictionary = call.target().dispatch_instance()?;
        self.resolver().resolve_continuation_dictionary(dictionary).await
    }

    /// The type of the frame that the callee of `call` runs in, when `call`
    /// suspends and is not a call of a continuation: the caller holds that
    /// frame while it waits.
    pub(super) async fn callee_frame_type(&self, call: &IRCall) -> Interned<MonoType> {
        let coroutine_ty = call
            .coroutine_ty(self.engine())
            .await
            .expect("a call of an `extern def` does not suspend");
        self.resolver().lower_type(&coroutine_ty).await
    }

    /// Calls a continuation: resumes it with the only element of the argument
    /// tuple of `Def.call` in a poll loop at `site`, and continues in `done`
    /// with its result in `destination`.
    pub(super) async fn lower_continuation_call(
        &mut self,
        call: &IRCall,
        instance: MonoCoroutineInstance,
        site: PollSite,
        destination: Place,
        done: BlockID,
        ctx: &Context<'_>,
    ) {
        let [continuation, tuple] = call.arguments() else {
            panic!("`Def.call` receives the continuation and an argument tuple")
        };

        // Resume the continuation with the only element of the tuple.
        let value_type = ctx.expression_type(*tuple).assert_as_tuple().fields()[0].clone();
        let tuple = ctx.expression_place(*tuple);
        let resumee = Resumee::Continuation {
            continuation: ctx.expression_place(*continuation),
            continuation_type: ctx.expression_type(*continuation).clone(),
            value: Operand::Copy(self.project(&tuple).tuple_field(FieldIndex::new(0))),
            value_type,
        };
        let poll = self.begin_poll(site, instance, resumee, ctx).await;

        // Every operation is forwarded.
        let return_arm = self.return_arm(&poll, destination, done);
        self.finish_poll(&poll, return_arm, &[], ctx);
    }

    /// Lowers a call that suspends at `point`: the callee runs in a new frame
    /// in the frame of this coroutine, whose operations are forwarded until it
    /// returns. It then continues in `done` with its result in `destination`.
    ///
    /// A callee that holds this coroutine's frame in turn runs in a box
    /// instead, which the frame points to while the call lasts.
    pub(super) async fn lower_new_frame_call(
        &mut self,
        point: SuspensionPointID,
        call: &IRCall,
        destination: Place,
        done: BlockID,
        ctx: &Context<'_>,
    ) {
        let frame_type = self.callee_frame_type(call).await;
        let instance = frame_type.assert_as_coroutine_frame().clone();

        // Create the frame of the callee where this coroutine holds what it
        // waits for, in a box if it is boxed.
        let heap_box = self.allocate_child_frame_box(point, ctx);
        let frame = ctx.child_frame_place(point);
        let arguments = call.arguments().iter().map(|argument| ctx.expression_place(*argument));
        self.initialize_coroutine(&frame, &instance, arguments).await;

        // Resume the new frame until it returns, forwarding every operation.
        let resumee = Resumee::NewFrame { frame, frame_type, heap_box };
        let poll = self.begin_poll(PollSite::Suspending(point), instance, resumee, ctx).await;
        let return_arm = self.return_arm(&poll, destination, done);
        self.finish_poll(&poll, return_arm, &[], ctx);
    }
}
