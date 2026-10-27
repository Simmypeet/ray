use std::collections::hash_map::Entry;

use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{
    borrowed_locals::MaybeBorrowedLocals,
    ir_expr::IRExprID,
    liveness::{expr::ExprLiveness, local::LocalLiveness},
};
use rayc_solver::Solver;

use crate::{layout_context::LayoutContext, savable::Savable, suspension_point::SuspensionPointID};

/// Specifies where a savable value is placed in a coroutine frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Placement {
    /// The value is stored in the coroutine prefix part.
    Prefix,
    SuspensionLocal(SuspensionLocalPlacement),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum SuspensionLocalPlacement {
    /// Right at the start of the coroutine, after it's created, but before it
    /// has been resumed for the first time.
    Start,
    /// The value is stored in the coroutine suspension local part.
    SuspensionLocal(SuspensionPointID),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct SuspensionLocalLayout {
    awaitee: Option<IRExprID>,
}

/// The layout of a coroutine frame, which stores the values that a coroutine
/// function has to hold across its suspension points.
///
/// Currently, the coroutine frame layout follows similar model as Rust's
/// coroutine frame layout.
///
/// The coroutine frame layout is divided into two parts: the prefix part and
/// the suspension local part.
#[derive(Debug, Clone, PartialEq, Eq, StableHash, Encode, Decode)]
pub struct FrameLayout {
    placements: FxHashMap<Savable, Placement>,
    suspension_locals: FxHashMap<SuspensionLocalPlacement, SuspensionLocalLayout>,
}

impl LayoutContext<'_> {
    async fn compute_initial_frame_layout(&self, solver: &mut Solver) -> FrameLayout {
        let points = self.calculate_waitee_for_each_suspension_point(solver).await;

        let local_liveness = LocalLiveness::compute(self.ir_function()).await;
        let expr_liveness = ExprLiveness::compute(self.ir_function()).await;
        let maybe_borrowed_locals = MaybeBorrowedLocals::compute(self.ir_function()).await;

        let mut layout = FrameLayout {
            placements: FxHashMap::default(),
            suspension_locals: FxHashMap::default(),
        };

        // initialize parameters to be placed in the
        // SuspensionLocalPlacement::Start part
        for parameter in self.parameter_locals().await {
            layout.placements.insert(
                Savable::Local(parameter),
                Placement::SuspensionLocal(SuspensionLocalPlacement::Start),
            );
        }

        for (&suspension_point_id, &waitee_expr_id) in &points {
            let saved_locals = local_liveness
                .block_exit(suspension_point_id.block_id())
                .expect("should exist")
                .live()
                .map(Savable::Local);
            let saved_exprs = expr_liveness
                .block_exit(suspension_point_id.block_id())
                .expect("should exist")
                .live()
                .map(Savable::Expression);
            let saved_borrowed_locals = maybe_borrowed_locals
                .block_exit(suspension_point_id.block_id())
                .expect("should exist")
                .iter()
                .map(Savable::Local);

            for savable in saved_locals.chain(saved_exprs).chain(saved_borrowed_locals) {
                match layout.placements.entry(savable) {
                    // it has been placed elsewhere, need to promote it to the prefix part
                    Entry::Occupied(mut occupied_entry) => {
                        *occupied_entry.get_mut() = Placement::Prefix;
                    }

                    // The value is not yet placed in the coroutine frame. place it exclusively in
                    // the suspension local part.
                    Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(Placement::SuspensionLocal(
                            SuspensionLocalPlacement::SuspensionLocal(suspension_point_id),
                        ));
                    }
                }
            }

            // set the suspension local layout for this suspension point
            layout.suspension_locals.insert(
                SuspensionLocalPlacement::SuspensionLocal(suspension_point_id),
                SuspensionLocalLayout { awaitee: Some(waitee_expr_id) },
            );
        }

        todo!()
    }
}
