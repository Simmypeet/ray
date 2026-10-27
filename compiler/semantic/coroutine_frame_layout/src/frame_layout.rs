use qbice::{Decode, Encode, StableHash};
use rayc_hash::FxHashMap;
use rayc_ir::{cfg::BlockID, ir_expr::IRExprID};

use crate::savable::Savable;

/// Specifies where a savable value is placed in a coroutine frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode)]
pub enum Placement {
    /// The value is stored in the coroutine prefix part.
    Prefix,
    SuspensionLocal(BlockID),
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
    suspension_locals: FxHashMap<BlockID, SuspensionLocalLayout>,
}
