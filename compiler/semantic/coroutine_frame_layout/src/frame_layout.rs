use std::collections::hash_map::Entry;

use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{self, get_ir, ir_expr::IRExprID, ir_function::IRFunctionID};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

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

impl FrameLayout {
    pub async fn compute(
        engine: TrackedEngine,
        solver: &mut Solver,
        def_id: GlobalSymbolID,
        local_ir_function_id: IRFunctionID,
        subst: Subst,
    ) -> Self {
        let ir_function_map = engine.get_ir(def_id).await;
        let ir_function = ir_function_map.get_function(local_ir_function_id);
        let layout_context = LayoutContext::new(subst, engine, def_id, ir_function).await;

        layout_context.compute_initial_frame_layout(solver).await
    }
}

impl LayoutContext<'_> {
    async fn compute_initial_frame_layout(&self, solver: &mut Solver) -> FrameLayout {
        let points = self.calculate_waitee_for_each_suspension_point(solver).await;

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
            for savable in self.savables_at(suspension_point_id.block_id()) {
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

        layout
    }
}
