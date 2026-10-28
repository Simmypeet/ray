use std::collections::hash_map::Entry;

use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{self, get_ir, ir_expr::IRExprID, ir_function::IRFunctionID};
use rayc_qbice::TrackedEngine;
use rayc_solver::Solver;
use rayc_symbol::GlobalSymbolID;
use rayc_type::subst::Subst;

use crate::{
    candidate_index::CandidateIndex, layout_context::LayoutContext, savable::Savable,
    suspension_point::SuspensionPointID,
};

/// Specifies where a savable value is placed in a coroutine frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Placement {
    /// The value is stored in the coroutine prefix part.
    Prefix,
    SuspensionLocal(SuspensionLocalPlacement),
}

impl Placement {
    /// Returns the suspension point whose variant alone holds the value, or
    /// `None` when the value is in the prefix or in the start variant.
    ///
    /// Only such a value may share memory with a value of another variant.
    #[must_use]
    pub const fn suspension_point(&self) -> Option<SuspensionPointID> {
        match self {
            Self::SuspensionLocal(SuspensionLocalPlacement::SuspensionLocal(
                suspension_point_id,
            )) => Some(*suspension_point_id),
            Self::Prefix | Self::SuspensionLocal(SuspensionLocalPlacement::Start) => None,
        }
    }
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
        let layout_context =
            LayoutContext::new(subst, engine, def_id, local_ir_function_id, &ir_function_map).await;

        // TODO: Has to do conflict resolution and upgrade the placement
        layout_context.compute_initial_frame_layout(solver).await
    }

    /// Numbers the values placed in exactly one suspension variant: the ones
    /// that may share memory with a value of another variant.
    ///
    /// Read from the initial placements, before any conflict promotion.
    #[must_use]
    pub fn candidate_index(&self) -> CandidateIndex {
        CandidateIndex::new(
            self.placements
                .iter()
                .filter_map(|(savable, placement)| placement.suspension_point().map(|_| *savable)),
        )
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

        // initialize captures
        for captures in self.capture_savables().into_iter().flatten() {
            // if the captures are borrowed, this means that the function will
            // be invoked multiple times and captures should be survived across
            // multiple invocations.
            let placement = if self.borrows_captures() {
                Placement::Prefix
            } else {
                Placement::SuspensionLocal(SuspensionLocalPlacement::Start)
            };

            layout.placements.insert(captures, placement);
        }

        for (&suspension_point_id, &waitee_expr_id) in &points {
            for savable in self.savables_at(suspension_point_id.block_id()) {
                let local_suspension = Placement::SuspensionLocal(
                    SuspensionLocalPlacement::SuspensionLocal(suspension_point_id),
                );

                match layout.placements.entry(savable) {
                    // it has been placed elsewhere, need to promote it to the prefix part
                    Entry::Occupied(mut occupied_entry) => {
                        if occupied_entry.get_mut() != &local_suspension {
                            *occupied_entry.get_mut() = Placement::Prefix;
                        }
                    }

                    // The value is not yet placed in the coroutine frame. place it exclusively in
                    // the suspension local part.
                    Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(local_suspension);
                    }
                }
            }

            // the awaitee is only held at its own suspension point, so it is
            // always placed in its variant
            if waitee_expr_id.is_some() {
                layout.placements.insert(
                    Savable::Awaitee(suspension_point_id),
                    Placement::SuspensionLocal(SuspensionLocalPlacement::SuspensionLocal(
                        suspension_point_id,
                    )),
                );
            }

            // set the suspension local layout for this suspension point
            layout.suspension_locals.insert(
                SuspensionLocalPlacement::SuspensionLocal(suspension_point_id),
                SuspensionLocalLayout { awaitee: waitee_expr_id },
            );
        }

        layout
    }
}
