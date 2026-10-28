use derive_more::From;
use qbice::{Decode, Encode, StableHash};
use rayc_ir::{address::Local, ir_expr::IRExprID};

use crate::suspension_point::SuspensionPointID;

/// The values that can be saved into a coroutine frame
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, StableHash, Encode, Decode, From,
)]
pub enum Savable {
    Local(Local),
    Expression(IRExprID),

    /// What the function waits for while suspended at the suspension point:
    /// the callee's frame of a call, the frames of the handled body and its
    /// handlers of a `run ... with`, or the continuation of a forwarding
    /// `handle`. A `perform` has none.
    Awaitee(SuspensionPointID),
}
