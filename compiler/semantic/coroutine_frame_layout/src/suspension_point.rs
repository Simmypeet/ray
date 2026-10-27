use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{
    cfg::BlockID, get_ir, ir_expr::IRExprID, ir_function::IRFunctionID, suspension_point::Waitee,
};
use rayc_symbol::GlobalSymbolID;

use crate::layout_context::LayoutContext;

/// Used to identify a suspension point in the IR. Typically, it's the terminal
/// of a particular block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub struct SuspensionPointID(BlockID);

impl SuspensionPointID {
    #[must_use]
    pub const fn block_id(&self) -> BlockID { self.0 }
}

impl LayoutContext {
    /// Determines whether a waitee still counts as a suspension point after the
    /// substitution is applied to it. This is typically due to when an
    /// expression emits an opaque effect variable that is later substituted
    /// to an empty effect row, which means that the expression is no longer
    /// a suspension point.
    pub(crate) async fn is_waitee_still_a_suspension_point(&mut self, waitee: Waitee<'_>) -> bool {
        let effect_row = match waitee {
            Waitee::Perform => return true,
            Waitee::Call(call) => call.callee_effect_row(self.engine()).await,
            Waitee::RunWith(run_with) => run_with.residual_effect().clone(),
            Waitee::Handle(handle) => handle.forwarded_effect().clone(),
        };

        !self.latest_type(&effect_row).await.is_empty_effect_row()
    }

    /// Calculates the waitee expression for each suspension point in the given
    /// function.
    pub(crate) async fn calculate_waitee_for_each_suspension_point(
        &mut self,
        def_id: GlobalSymbolID,
        local_ir_function_id: IRFunctionID,
    ) -> FxHashMap<SuspensionPointID, IRExprID> {
        let mut suspensions = FxHashMap::default();
        let ir_function_map = self.engine().get_ir(def_id).await;
        let ir_function = ir_function_map.get_function(local_ir_function_id);

        for (block_id, block) in ir_function.traverse() {
            let Some(terminator) = block.terminator() else {
                continue;
            };

            // the suspension point and the terminator
            let Some(waitee_expr_id) = terminator.suspension_waitee() else {
                continue;
            };

            let waitee = Waitee::of(ir_function.get_expression(waitee_expr_id).kind());
            let is_suspension_point = self.is_waitee_still_a_suspension_point(waitee).await;

            // is still considered as a suspension point after the substitution
            // is applied
            if is_suspension_point {
                suspensions.insert(SuspensionPointID(block_id), waitee_expr_id);
            }
        }

        suspensions
    }
}
