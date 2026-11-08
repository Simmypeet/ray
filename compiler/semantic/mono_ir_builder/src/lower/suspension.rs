use rayc_coroutine_frame_layout::suspension_point::SuspensionPointID;
use rayc_ir::{cfg::Suspend, suspension_point::Waitee};

use super::poll::PollSite;
use crate::{context::Context, emitter::Emitter};

impl Emitter<'_> {
    /// Lowers a suspension point of a coroutine, according to what it waits
    /// for.
    pub(super) async fn lower_suspension(
        &mut self,
        point: SuspensionPointID,
        suspend: &Suspend,
        ctx: &Context<'_>,
    ) {
        let kind = ctx.source_expression(suspend.expression()).kind();
        match Waitee::of(kind) {
            Waitee::Perform => self.lower_yield_perform(point, suspend, ctx).await,
            Waitee::Call(call) => {
                let done = self.lower_edge(point.block_id(), suspend.resume_block(), ctx);
                let destination = ctx.expression_place(suspend.expression());

                // A continuation already has its frame somewhere else, while
                // any other callee gets a new one in this coroutine's frame.
                if let Some(instance) = self.resumed_continuation(call).await {
                    let site = PollSite::Suspending(point);
                    self.lower_continuation_call(call, instance, site, destination, done, ctx)
                        .await;
                } else {
                    self.lower_new_frame_call(point, call, destination, done, ctx).await;
                }
            }
            Waitee::RunWith(run_with) => {
                self.lower_run_with_suspension(point, suspend, run_with, ctx).await;
            }
            Waitee::Handle(_) => panic!("a `handle` is evaluated by a `Handle` terminator"),
        }
    }
}
